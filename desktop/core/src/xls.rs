//! .xls（BIFF8 / OLE2 复合文档）读取：WPS 云表格只导出这个格式，而解析器只吃 xlsx。
//!
//! 只解「值 + 合并单元格」两样——出货清单要的就是这两样：公式直接用 FORMULA 记录里
//! 缓存的结果（不重算），字体样式 / 日期格式 / 图表一律不管。
//!
//! 不依赖任何 crate：拉不到 calamine 的场景（离线构建）也得能用，而且只想为一个
//! 格式多背一份依赖不划算。
//!
//! 结构：CFB 复合文档 → `Workbook` 流 → BIFF8 记录 → 二维表。

use std::collections::HashMap;

/// 扇区号 >= 这个值都表示「链结束 / 特殊标记」。
const END: u32 = 0xFFFF_FFFA;
const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

const REC_EOF: u16 = 0x000A;
const REC_FORMULA: u16 = 0x0006;
const REC_BOUNDSHEET: u16 = 0x0085;
const REC_MULRK: u16 = 0x00BD;
const REC_MERGEDCELLS: u16 = 0x00E5;
const REC_SST: u16 = 0x00FC;
const REC_LABELSST: u16 = 0x00FD;
const REC_NUMBER: u16 = 0x0203;
const REC_BOOLERR: u16 = 0x0205;
const REC_RK: u16 = 0x027E;
const REC_CONTINUE: u16 = 0x003C;

// ---------- 小工具 ----------

fn rd_u16(d: &[u8], p: usize) -> Option<u16> {
    d.get(p..p + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn rd_u32(d: &[u8], p: usize) -> Option<u32> {
    d.get(p..p + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn rd_f64(d: &[u8], p: usize) -> Option<f64> {
    d.get(p..p + 8)
        .map(|b| f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

fn decode_utf16le(raw: &[u8]) -> String {
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// BIFF 里的「压缩字符串」（每字节一个字符）按 latin-1 处理。
fn decode_latin1(raw: &[u8]) -> String {
    raw.iter().map(|b| *b as char).collect()
}

/// 数字转单元格文本：整数别写成 `10.0`（`总箱数` 要能被解析器读成整数）。
fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return String::new();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn rk_value(rk: u32) -> f64 {
    let mut v = if rk & 0x02 != 0 {
        // 高 30 位是有符号整数（左移过 2 位的 30 位补码）
        let raw = ((rk >> 2) & 0x3FFF_FFFF) as i32;
        let signed = if raw & 0x2000_0000 != 0 {
            raw - 0x4000_0000
        } else {
            raw
        };
        signed as f64
    } else {
        f64::from_bits(((rk & 0xFFFF_FFFC) as u64) << 32)
    };
    if rk & 0x01 != 0 {
        v /= 100.0;
    }
    v
}

// ---------- CFB（OLE2 复合文档） ----------

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    etype: u8,
    start: u32,
    size: u64,
}

struct Cfb {
    data: Vec<u8>,
    sector: usize,
    mini_size: usize,
    cut_off: u32,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    mini_stream: Vec<u8>,
    entries: Vec<Entry>,
}

impl Cfb {
    fn open(path: &str) -> Result<Cfb, String> {
        let data = std::fs::read(path).map_err(|e| format!("读不了文件 {path}：{e}"))?;
        if data.len() < 512 {
            return Err("文件太小，不是 .xls".into());
        }
        if data[..8] != CFB_MAGIC {
            return Err("不是 .xls（OLE 复合文档）文件".into());
        }
        let sector = 1usize << rd_u16(&data, 30).unwrap_or(9);
        let mini_size = 1usize << rd_u16(&data, 32).unwrap_or(6);
        let num_fat = rd_u32(&data, 44).unwrap_or(0);
        let dir_start = rd_u32(&data, 48).unwrap_or(0);
        let cut_off = rd_u32(&data, 56).unwrap_or(4096);
        let mini_fat_start = rd_u32(&data, 60).unwrap_or(0);
        if sector < 128 || sector > 65536 {
            return Err(format!("扇区大小异常：{sector}"));
        }

        let mut cfb = Cfb {
            data,
            sector,
            mini_size,
            cut_off,
            fat: Vec::new(),
            mini_fat: Vec::new(),
            mini_stream: Vec::new(),
            entries: Vec::new(),
        };

        // FAT 扇区表：头里 109 个 + DIFAT 链上的
        let mut fat_sectors: Vec<u32> = (0..109)
            .filter_map(|i| rd_u32(&cfb.data, 76 + 4 * i))
            .collect();
        let mut next = rd_u32(&cfb.data, 68).unwrap_or(END);
        let mut guard = 0;
        while next < END && guard < 4096 {
            let off = cfb.sec_off(next);
            let per = cfb.sector / 4;
            if off + per * 4 > cfb.data.len() {
                break;
            }
            let mut vals: Vec<u32> = (0..per)
                .filter_map(|k| rd_u32(&cfb.data, off + 4 * k))
                .collect();
            if let Some(last) = vals.pop() {
                fat_sectors.extend(vals);
                next = last;
            } else {
                break;
            }
            guard += 1;
        }
        fat_sectors.retain(|s| *s < END);
        fat_sectors.truncate(num_fat as usize);

        for s in &fat_sectors {
            let off = cfb.sec_off(*s);
            let per = cfb.sector / 4;
            if off + per * 4 > cfb.data.len() {
                continue;
            }
            for k in 0..per {
                if let Some(v) = rd_u32(&cfb.data, off + 4 * k) {
                    cfb.fat.push(v);
                }
            }
        }

        // 目录项
        let dir_bytes = cfb.chain_bytes(dir_start);
        for chunk in dir_bytes.chunks_exact(128) {
            let nlen = rd_u16(chunk, 64).unwrap_or(0) as usize;
            let name = if nlen >= 2 && nlen - 2 <= 64 {
                decode_utf16le(&chunk[..nlen - 2])
            } else {
                String::new()
            };
            cfb.entries.push(Entry {
                name,
                etype: chunk.get(66).copied().unwrap_or(0),
                start: rd_u32(chunk, 116).unwrap_or(END),
                size: rd_u32(chunk, 120).unwrap_or(0) as u64,
            });
        }

        if mini_fat_start < END {
            let mf = cfb.chain_bytes(mini_fat_start);
            cfb.mini_fat = mf.chunks_exact(4).filter_map(|c| rd_u32(c, 0)).collect();
            if let Some(root) = cfb.entries.iter().find(|e| e.etype == 5) {
                let start = root.start;
                let size = root.size as usize;
                let mut stream = cfb.chain_bytes(start);
                stream.truncate(size);
                cfb.mini_stream = stream;
            }
        }
        Ok(cfb)
    }

    fn sec_off(&self, s: u32) -> usize {
        512 + (s as usize) * self.sector
    }

    fn sec_bytes(&self, s: u32) -> &[u8] {
        let off = self.sec_off(s);
        let end = (off + self.sector).min(self.data.len());
        if off >= self.data.len() {
            &[]
        } else {
            &self.data[off..end]
        }
    }

    /// FAT 链 → 扇区号列表（带环检测与上限）。
    fn chain(&self, start: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut s = start;
        let mut seen: Vec<u32> = Vec::new();
        while s < END && out.len() < 400_000 {
            if seen.contains(&s) || s as usize >= self.fat.len() {
                break;
            }
            seen.push(s);
            out.push(s);
            s = self.fat[s as usize];
        }
        out
    }

    fn chain_bytes(&self, start: u32) -> Vec<u8> {
        let mut out = Vec::new();
        for s in self.chain(start) {
            out.extend_from_slice(self.sec_bytes(s));
        }
        out
    }

    fn stream(&self, name: &str) -> Result<Vec<u8>, String> {
        let e = self
            .entries
            .iter()
            .find(|e| e.etype == 2 && e.name == name)
            .ok_or_else(|| format!("这个 .xls 里没有 {name} 流"))?;
        let size = e.size as usize;
        let mut out: Vec<u8> = if (e.size as u32) < self.cut_off && !self.mini_fat.is_empty() {
            let mut buf = Vec::new();
            let mut s = e.start;
            let mut seen: Vec<u32> = Vec::new();
            while s < END && (s as usize) < self.mini_fat.len() && !seen.contains(&s) {
                seen.push(s);
                let off = (s as usize) * self.mini_size;
                let end = (off + self.mini_size).min(self.mini_stream.len());
                if off >= self.mini_stream.len() {
                    break;
                }
                buf.extend_from_slice(&self.mini_stream[off..end]);
                s = self.mini_fat[s as usize];
            }
            buf
        } else {
            self.chain_bytes(e.start)
        };
        out.truncate(size);
        Ok(out)
    }
}

// ---------- BIFF8 记录 ----------

struct Records<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Records<'a> {
    fn new(data: &'a [u8], pos: usize) -> Records<'a> {
        Records { data, pos }
    }

    fn next(&mut self) -> Option<(u16, &'a [u8])> {
        let rid = rd_u16(self.data, self.pos)?;
        let len = rd_u16(self.data, self.pos + 2)? as usize;
        let start = self.pos + 4;
        let end = (start + len).min(self.data.len());
        self.pos = start + len;
        Some((rid, &self.data[start..end]))
    }
}

/// 读 SST（共享字符串表）+ 后续 CONTINUE。`segments` 是按记录切开的分段。
fn read_sst(segments: &[&[u8]]) -> Vec<String> {
    let mut strings: Vec<String> = Vec::new();
    if segments.is_empty() {
        return strings;
    }
    let mut si = 0usize;
    let mut buf: &[u8] = segments[0];
    let mut p = 8usize; // 跳过 cstTotal / cstUnique

    // 换到下一段（CONTINUE 段的第 0 字节是新串的 grbit）

    loop {
        if p >= buf.len() {
            si += 1;
            if si >= segments.len() {
                break;
            }
            buf = segments[si];
            p = 0;
        }
        if p + 3 > buf.len() {
            break;
        }
        let cch = rd_u16(buf, p).unwrap_or(0) as usize;
        let grbit = buf[p + 1 + 1];
        p += 3;
        let high = grbit & 0x01 != 0;
        let rich = grbit & 0x08 != 0;
        let ext = grbit & 0x04 != 0;
        let mut c_run = 0usize;
        let mut cb_ext = 0usize;
        if rich {
            if p + 2 > buf.len() {
                break;
            }
            c_run = rd_u16(buf, p).unwrap_or(0) as usize;
            p += 2;
        }
        if ext {
            if p + 4 > buf.len() {
                break;
            }
            cb_ext = rd_u32(buf, p).unwrap_or(0) as usize;
            p += 4;
        }

        let mut high_now = high;
        let mut out = String::new();
        let mut need = cch;
        while need > 0 {
            let width = if high_now { 2 } else { 1 };
            let avail = (buf.len() - p) / width;
            if avail == 0 {
                si += 1;
                if si >= segments.len() {
                    break;
                }
                buf = segments[si];
                high_now = buf.first().map(|b| b & 0x01 != 0).unwrap_or(false);
                p = 1;
                continue;
            }
            let take = need.min(avail);
            let bytes = &buf[p..p + take * width];
            if high_now {
                out.push_str(&decode_utf16le(bytes));
            } else {
                out.push_str(&decode_latin1(bytes));
            }
            p += take * width;
            need -= take;
            if need > 0 {
                si += 1;
                if si >= segments.len() {
                    break;
                }
                buf = segments[si];
                high_now = buf.first().map(|b| b & 0x01 != 0).unwrap_or(false);
                p = 1;
            }
        }
        strings.push(out);

        if rich {
            p += 4 * c_run;
        }
        if ext {
            p += cb_ext;
        }
        while p > buf.len() {
            si += 1;
            if si >= segments.len() {
                break;
            }
            buf = segments[si];
            p = 0;
        }
        if si >= segments.len() {
            break;
        }
    }
    strings
}

/// BOUNDSHEET 记录 → (Workbook 流里的偏移, 表名)。
fn boundsheets(recs: &[(u16, Vec<u8>)]) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for (rid, body) in recs {
        if *rid != REC_BOUNDSHEET || body.len() < 8 {
            continue;
        }
        let off = rd_u32(body, 0).unwrap_or(0);
        let cch = body[6] as usize;
        let flags = body[7];
        let raw = body.get(8..).unwrap_or(&[]);
        let name = if flags & 0x01 != 0 {
            decode_utf16le(&raw[..(cch * 2).min(raw.len())])
        } else {
            decode_latin1(&raw[..cch.min(raw.len())])
        };
        out.push((off, name));
    }
    out
}

fn collect_records(data: &[u8]) -> Vec<(u16, Vec<u8>)> {
    let mut out = Vec::new();
    let mut r = Records::new(data, 0);
    while let Some((rid, body)) = r.next() {
        out.push((rid, body.to_vec()));
        if out.len() > 2_000_000 {
            break;
        }
    }
    out
}

// ---------- 对外接口 ----------

/// 一张工作表：已把合并单元格铺开（左上角的值复制到整个合并区）。
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

impl Sheet {
    /// 非空行数。
    pub fn nonempty_rows(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.iter().any(|c| !c.is_empty()))
            .count()
    }

    /// 真有数据的行（≥ 4 个非空格）：用来区分「只有表头的空模板」。
    pub fn rich_rows(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.iter().filter(|c| !c.is_empty()).count() >= 4)
            .count()
    }

    /// 前 6 行里出现「收货仓 / 来货日期 / 入库号」→ 认成来货登记表。
    pub fn look_like_packing(&self) -> bool {
        const KEYS: [&str; 3] = ["收货仓", "来货日期", "入库号"];
        self.rows.iter().take(6).any(|row| {
            row.iter()
                .any(|c| KEYS.iter().any(|k| c.contains(k)))
        })
    }
}

/// 打开一次、按需读表：kdocs 那本 44 张表全读要十几秒，按需读一张不到 1 秒。
pub struct Book {
    wb: Vec<u8>,
    names: Vec<(u32, String)>,
    sst: Vec<String>,
}

impl Book {
    pub fn open(path: &str) -> Result<Book, String> {
        let cfb = Cfb::open(path)?;
        let wb = cfb.stream("Workbook")?;
        let recs = collect_records(&wb);
        let names = boundsheets(&recs);

        // SST：一条 REC_SST 后面跟着若干 CONTINUE
        let mut sst = Vec::new();
        for (i, (rid, _)) in recs.iter().enumerate() {
            if *rid == REC_SST {
                let mut segments: Vec<&[u8]> = vec![&recs[i].1];
                let mut j = i + 1;
                while j < recs.len() && recs[j].0 == REC_CONTINUE {
                    segments.push(&recs[j].1);
                    j += 1;
                }
                sst = read_sst(&segments);
                break;
            }
        }

        Ok(Book {
            wb,
            names,
            sst,
        })
    }

    pub fn names(&self) -> Vec<String> {
        self.names.iter().map(|(_, n)| n.clone()).collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.names.iter().any(|(_, n)| n.trim() == name.trim())
    }

    /// 读一张表（合并单元格已铺开）。
    pub fn sheet(&self, name: &str) -> Result<Sheet, String> {
        let (pos, real_name) = self
            .names
            .iter()
            .find(|(_, n)| n.trim() == name.trim())
            .map(|(p, n)| (*p, n.clone()))
            .ok_or_else(|| {
                format!(
                    "这个 .xls 里没有表「{name}」，可选：{}",
                    self.names
                        .iter()
                        .map(|(_, n)| n.as_str())
                        .collect::<Vec<_>>()
                        .join("、")
                )
            })?;

        let mut cells: HashMap<(usize, usize), String> = HashMap::new();
        let mut merges: Vec<(usize, usize, usize, usize)> = Vec::new();
        let mut r = Records::new(&self.wb, pos as usize);
        while let Some((rid, body)) = r.next() {
            if rid == REC_EOF {
                break;
            }
            match rid {
                REC_LABELSST => {
                    if body.len() >= 10 {
                        let row = rd_u16(body, 0).unwrap_or(0) as usize;
                        let col = rd_u16(body, 2).unwrap_or(0) as usize;
                        let isst = rd_u32(body, 6).unwrap_or(0) as usize;
                        let text = self.sst.get(isst).cloned().unwrap_or_default();
                        cells.insert((row, col), text);
                    }
                }
                REC_NUMBER => {
                    if body.len() >= 14 {
                        if let (Some(row), Some(col), Some(v)) = (
                            rd_u16(body, 0).map(usize::from),
                            rd_u16(body, 2).map(usize::from),
                            rd_f64(body, 6),
                        ) {
                            cells.insert((row, col), fmt_num(v));
                        }
                    }
                }
                REC_RK => {
                    if body.len() >= 10 {
                        if let (Some(row), Some(col), Some(rk)) = (
                            rd_u16(body, 0).map(usize::from),
                            rd_u16(body, 2).map(usize::from),
                            rd_u32(body, 6),
                        ) {
                            cells.insert((row, col), fmt_num(rk_value(rk)));
                        }
                    }
                }
                REC_MULRK => {
                    if body.len() >= 6 {
                        let row = rd_u16(body, 0).unwrap_or(0) as usize;
                        let col0 = rd_u16(body, 2).unwrap_or(0) as usize;
                        let n = (body.len() - 6) / 6;
                        for k in 0..n {
                            if let Some(rk) = rd_u32(body, 4 + k * 6 + 2) {
                                cells.insert((row, col0 + k), fmt_num(rk_value(rk)));
                            }
                        }
                    }
                }
                REC_BOOLERR => {
                    if body.len() >= 8 {
                        let row = rd_u16(body, 0).unwrap_or(0) as usize;
                        let col = rd_u16(body, 2).unwrap_or(0) as usize;
                        let val = body[6];
                        let is_err = body[7];
                        if is_err == 0 {
                            cells.insert((row, col), if val != 0 { "TRUE" } else { "FALSE" }.into());
                        }
                    }
                }
                REC_FORMULA => {
                    if body.len() >= 20 {
                        if let (Some(row), Some(col)) = (
                            rd_u16(body, 0).map(usize::from),
                            rd_u16(body, 2).map(usize::from),
                        ) {
                            let res = &body[6..14];
                            if res[6] == 0xFF && res[7] == 0xFF {
                                // 特殊值：0=字符串（值在下一条 STRING 记录）1=布尔 2=错误 3=空
                                if res[0] == 1 {
                                    cells.insert(
                                        (row, col),
                                        if res[2] != 0 { "TRUE" } else { "FALSE" }.into(),
                                    );
                                }
                            } else {
                                let v = rd_f64(res, 0).unwrap_or(0.0);
                                cells.insert((row, col), fmt_num(v));
                            }
                        }
                    }
                }
                REC_MERGEDCELLS => {
                    if body.len() >= 2 {
                        let cnt = rd_u16(body, 0).unwrap_or(0) as usize;
                        for k in 0..cnt {
                            let p = 2 + k * 8;
                            if p + 8 > body.len() {
                                break;
                            }
                            let r1 = rd_u16(body, p).unwrap_or(0) as usize;
                            let r2 = rd_u16(body, p + 2).unwrap_or(0) as usize;
                            let c1 = rd_u16(body, p + 4).unwrap_or(0) as usize;
                            let c2 = rd_u16(body, p + 6).unwrap_or(0) as usize;
                            merges.push((r1, c1, r2, c2));
                        }
                    }
                }
                _ => {}
            }
        }

        // 合并单元格：左上角的值铺满整个区域（和 xlsx 那条路的 apply_merges 一致）
        for (r1, c1, r2, c2) in &merges {
            if r2 <= r1 && c2 <= c1 {
                continue;
            }
            let Some(v) = cells.get(&(*r1, *c1)).cloned() else {
                continue;
            };
            for row in *r1..=*r2 {
                for col in *c1..=*c2 {
                    cells.insert((row, col), v.clone());
                }
            }
        }

        let max_row = cells.keys().map(|(r, _)| *r).max();
        let max_col = cells.keys().map(|(_, c)| *c).max();
        let mut rows = Vec::new();
        if let (Some(max_row), Some(max_col)) = (max_row, max_col) {
            for r in 0..=max_row {
                let mut row = Vec::with_capacity(max_col + 1);
                for c in 0..=max_col {
                    row.push(cells.get(&(r, c)).cloned().unwrap_or_default());
                }
                rows.push(row);
            }
        }
        Ok(Sheet {
            name: real_name,
            rows,
        })
    }

    /// 从后往前挑第一张「像来货登记且真有数据」的表（表按月排，最后一张最新）。
    pub fn pick_packing(&self) -> Option<String> {
        let names = self.names();
        for name in names.iter().rev() {
            if let Ok(s) = self.sheet(name) {
                if s.look_like_packing() && s.rich_rows() >= 3 {
                    return Some(name.clone());
                }
            }
        }
        for name in names.iter().rev() {
            if let Ok(s) = self.sheet(name) {
                if s.look_like_packing() {
                    return Some(name.clone());
                }
            }
        }
        None
    }

    /// 只要表名（不解析单元格）。
    pub fn sheet_names(&self) -> Vec<String> {
        self.names()
    }

    /// 简化：读整本（调试 / 校验用）。
    pub fn all_sheets(&self) -> Vec<Sheet> {
        self.names()
            .iter()
            .filter_map(|n| self.sheet(n).ok())
            .collect()
    }
}

/// 文件是不是 .xls（按魔数判断，别信扩展名）。
pub fn is_xls(path: &str) -> bool {
    std::fs::File::open(path)
        .ok()
        .and_then(|mut f| {
            use std::io::Read as _;
            let mut head = [0u8; 8];
            f.read_exact(&mut head).ok().map(|_| head)
        })
        .map(|head| head == CFB_MAGIC)
        .unwrap_or(false)
}
