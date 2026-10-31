// xlsx / txt 读取：手写 zip + deflate + sheet XML 解析（不引外部 crate）

use std::collections::BTreeMap;
use std::io::Read as _;

use super::PackingError;

pub(crate) fn read_text_file(path: &str) -> Result<String, PackingError> {
    let bytes = std::fs::read(path)
        .map_err(|e| PackingError::new("io", format!("读取 {path} 失败：{e}")))?;
    if let Ok(s) = String::from_utf8(bytes.clone()) {
        return Ok(s);
    }
    // Excel 另存的 csv 在中文 Windows 上通常是 GBK
    let (decoded, _, _) = encoding_rs::GBK.decode(&bytes);
    Ok(decoded.into_owned())
}

pub(crate) fn read_xlsx(path: &str) -> Result<Vec<Vec<String>>, PackingError> {
    let bytes = std::fs::read(path)
        .map_err(|e| PackingError::new("io", format!("读取 {path} 失败：{e}")))?;
    let files = unzip(&bytes)?;

    let shared = files
        .get("xl/sharedStrings.xml")
        .map(|b| shared_strings(b))
        .unwrap_or_default();

    let mut sheet_names: Vec<&String> = files
        .keys()
        .filter(|n| n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml"))
        .collect();
    sheet_names.sort();
    let sheet_name = sheet_names
        .first()
        .ok_or_else(|| PackingError::new("xlsx", "xlsx 里没找到工作表"))?;
    let sheet_xml = &files[*sheet_name];

    let mut grid = sheet_rows(sheet_xml, &shared);
    if let Some(merges) = merge_ranges(sheet_xml) {
        apply_merges(&mut grid, &merges);
    }
    Ok(grid)
}

/// 合并单元格：把左上角的值铺满整个范围（Excel 里被合并的格子是空的）。
pub(crate) fn apply_merges(grid: &mut Vec<Vec<String>>, merges: &[(usize, usize, usize, usize)]) {
    for &(r1, c1, r2, c2) in merges {
        let value = get_cell(grid, r1, c1);
        if value.is_empty() {
            continue;
        }
        for r in r1..=r2 {
            for c in c1..=c2 {
                set_cell(grid, r, c, value.clone());
            }
        }
    }
}

pub(crate) fn get_cell(grid: &[Vec<String>], r: usize, c: usize) -> String {
    grid.get(r).and_then(|row| row.get(c)).cloned().unwrap_or_default()
}

pub(crate) fn set_cell(grid: &mut Vec<Vec<String>>, r: usize, c: usize, v: String) {
    while grid.len() <= r {
        grid.push(Vec::new());
    }
    let row = &mut grid[r];
    while row.len() <= c {
        row.push(String::new());
    }
    row[c] = v;
}

// ---------- 极简 zip + xlsx 读取 ----------
// 只依赖 flate2：xlsx 就是个 zip，里面 sheet1.xml + sharedStrings.xml。
// 不引 calamine（当前环境无法联网拉新 crate），够用即可。

pub(crate) fn unzip(data: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, PackingError> {
    let n = data.len();
    let mut eocd = None;
    let from = n.saturating_sub(66_000);
    let mut i = n.saturating_sub(22);
    loop {
        if i < from {
            break;
        }
        if data[i..i + 4] == *b"PK\x05\x06" {
            eocd = Some(i);
            break;
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }
    let eocd = eocd.ok_or_else(|| PackingError::new("xlsx", "不是有效的 xlsx（找不到 zip 目录）"))?;

    let count = u16::from_le_bytes([data[eocd + 10], data[eocd + 11]]) as usize;
    let cd_off = u32::from_le_bytes([
        data[eocd + 16],
        data[eocd + 17],
        data[eocd + 18],
        data[eocd + 19],
    ]) as usize;

    let mut out: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut p = cd_off;
    for _ in 0..count {
        if p + 46 > n || data[p..p + 4] != *b"PK\x01\x02" {
            break;
        }
        let method = u16::from_le_bytes([data[p + 10], data[p + 11]]);
        let csize = u32::from_le_bytes([data[p + 20], data[p + 21], data[p + 22], data[p + 23]]) as usize;
        let nlen = u16::from_le_bytes([data[p + 28], data[p + 29]]) as usize;
        let elen = u16::from_le_bytes([data[p + 30], data[p + 31]]) as usize;
        let clen = u16::from_le_bytes([data[p + 32], data[p + 33]]) as usize;
        let loff = u32::from_le_bytes([data[p + 42], data[p + 43], data[p + 44], data[p + 45]]) as usize;
        let name = String::from_utf8_lossy(&data[p + 46..(p + 46 + nlen).min(n)]).into_owned();

        if loff + 30 <= n {
            let lnlen = u16::from_le_bytes([data[loff + 26], data[loff + 27]]) as usize;
            let lelen = u16::from_le_bytes([data[loff + 28], data[loff + 29]]) as usize;
            let start = loff + 30 + lnlen + lelen;
            let end = (start + csize).min(n);
            if start < end {
                let raw = &data[start..end];
                let body = if method == 8 { inflate(raw)? } else { raw.to_vec() };
                out.insert(name, body);
            }
        }
        p += 46 + nlen + elen + clen;
    }
    Ok(out)
}

pub(crate) fn inflate(raw: &[u8]) -> Result<Vec<u8>, PackingError> {
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(raw)
        .read_to_end(&mut out)
        .map_err(|e| PackingError::new("xlsx", format!("解压失败：{e}")))?;
    Ok(out)
}

pub(crate) fn shared_strings(xml: &[u8]) -> Vec<String> {
    let s = String::from_utf8_lossy(xml).into_owned();
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(start) = s[pos..].find("<si") {
        let start = pos + start;
        let end = match s[start..].find("</si>") {
            Some(e) => start + e,
            None => break,
        };
        let block = &s[start..end];
        let mut text = String::new();
        let mut p = 0usize;
        while let Some(t) = block[p..].find("<t") {
            let t = p + t;
            if let Some(text_v) = inner_text(&block[t..], "t") {
                text.push_str(&unescape(&text_v));
            }
            p = t + 2;
        }
        out.push(text);
        pos = end + 5;
    }
    out
}

pub(crate) fn sheet_rows(xml: &[u8], shared: &[String]) -> Vec<Vec<String>> {
    let s = String::from_utf8_lossy(xml).into_owned();
    let mut grid: Vec<Vec<String>> = Vec::new();
    let mut pos = 0usize;

    while let Some(rs) = s[pos..].find("<row") {
        let row_start = pos + rs;
        let row_end = match s[row_start..].find("</row>") {
            Some(e) => row_start + e,
            None => break,
        };
        let row_xml = &s[row_start..row_end];
        let row_no = attr(row_xml, "r")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(grid.len() + 1);
        let r_idx = row_no.saturating_sub(1);

        let mut cp = 0usize;
        let mut col = 0usize;
        while let Some(cs) = row_xml[cp..].find("<c") {
            let c_start = cp + cs;
            let c_end = match row_xml[c_start..].find("</c>") {
                Some(e) => c_start + e + 4,
                None => break,
            };
            let c_xml = &row_xml[c_start..c_end];
            let c_idx = attr(c_xml, "r").map(|r| col_index(&r)).unwrap_or(col);
            let t = attr(c_xml, "t").unwrap_or_default();
            let value = if t == "s" {
                inner_text(c_xml, "v")
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .and_then(|i| shared.get(i).cloned())
                    .unwrap_or_default()
            } else if t == "inlineStr" {
                inner_text(c_xml, "is")
                    .map(|v| v)
                    .unwrap_or_default()
                    .pipe_strip_tags()
            } else {
                inner_text(c_xml, "v").unwrap_or_default()
            };
            set_cell(&mut grid, r_idx, c_idx, unescape(&value));
            col = c_idx + 1;
            cp = c_end;
        }
        pos = row_end + 6;
    }
    grid
}

/// `<is><t>abc</t></is>` → 取出里面所有 `<t>` 的文字。
trait StripTags {
    fn pipe_strip_tags(self) -> String;
}

impl StripTags for String {
    fn pipe_strip_tags(self) -> String {
        let mut out = String::new();
        let mut pos = 0usize;
        while let Some(t) = self[pos..].find("<t") {
            let t = pos + t;
            if let Some(v) = inner_text(&self[t..], "t") {
                out.push_str(&unescape(&v));
            }
            pos = t + 2;
        }
        if out.is_empty() {
            self
        } else {
            out
        }
    }
}

/// 取 `key="value"`（要求前面有空格，避免误配 `ref=` 之类）。
pub(crate) fn attr(xml: &str, key: &str) -> Option<String> {
    let pat = format!(" {key}=\"");
    let start = xml.find(&pat)? + pat.len();
    let rest = &xml[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 取 `<tag ...>text</tag>` 的 text。
pub(crate) fn inner_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = xml.find(&open)?;
    let after = &xml[start..];
    let gt = after.find('>')?;
    if after[..gt].ends_with('/') {
        return Some(String::new());
    }
    let body = &after[gt + 1..];
    let close = format!("</{tag}>");
    let end = body.find(&close)?;
    Some(body[..end].to_string())
}

/// "AB12" → 27（0 基列号）。
pub(crate) fn col_index(reference: &str) -> usize {
    let mut n = 0usize;
    for ch in reference.chars() {
        if ch.is_ascii_alphabetic() {
            n = n * 26 + (ch.to_ascii_uppercase() as usize - 'A' as usize + 1);
        } else {
            break;
        }
    }
    n.saturating_sub(1)
}

pub(crate) fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let end = tail.find(';').map(|e| e + 1).unwrap_or(1);
        let entity = &tail[..end];
        match entity {
            "&amp;" => out.push('&'),
            "&lt;" => out.push('<'),
            "&gt;" => out.push('>'),
            "&quot;" => out.push('"'),
            "&apos;" => out.push('\''),
            _ => {
                if let Some(num) = entity.strip_prefix("&#") {
                    let num = num.strip_suffix(';').unwrap_or(num);
                    let code = if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        num.parse::<u32>().ok()
                    };
                    if let Some(c) = code.and_then(char::from_u32) {
                        out.push(c);
                    }
                } else {
                    out.push_str(entity);
                }
            }
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// 合并单元格范围："A5:A7" → (4,0,6,0)。
pub(crate) fn merge_ranges(xml: &[u8]) -> Option<Vec<(usize, usize, usize, usize)>> {
    let s = String::from_utf8_lossy(xml).into_owned();
    let start = s.find("<mergeCells")?;
    let end = s[start..].find("</mergeCells>").map(|e| start + e).unwrap_or(s.len());
    let block = &s[start..end];

    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(m) = block[pos..].find("<mergeCell") {
        let m = pos + m;
        let seg = &block[m..];
        let ref_end = seg.find('>').map(|e| m + e).unwrap_or(block.len());
        if let Some(r) = attr(&block[m..ref_end + 1], "ref") {
            if let Some((a, b)) = r.split_once(':') {
                let (r1, c1) = split_ref(a);
                let (r2, c2) = split_ref(b);
                out.push((r1, c1, r2, c2));
            }
        }
        pos = ref_end + 1;
    }
    Some(out)
}

/// "A5" → (4, 0)
pub(crate) fn split_ref(reference: &str) -> (usize, usize) {
    let letters: String = reference.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let digits: String = reference.chars().skip_while(|c| c.is_ascii_alphabetic()).collect();
    let row = digits.parse::<usize>().unwrap_or(1).saturating_sub(1);
    (row, col_index(&letters))
}

// ---------- 测试 ----------

