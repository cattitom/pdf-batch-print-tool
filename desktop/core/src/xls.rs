//! .xls（BIFF8 / OLE2 复合文档）读取：WPS 云表格只导出这个格式，而解析器只吃 xlsx。
//!
//! 解「值 + 合并单元格 + 字体颜色」三样：出货清单要的前两样，公式直接用 FORMULA 记录里
//! 缓存的结果（不重算）；字体颜色只取到「是不是红的」这一层——云表格里红色字体表示
//! 已出货，程序照抄原表的判断，不再让人手点。日期格式 / 边框 / 图表仍然不管。
//!
//! 不依赖任何 crate：拉不到 calamine 的场景（离线构建）也得能用，而且只想为一个
//! 格式多背一份依赖不划算。
//!
//! 结构：CFB 复合文档 → `Workbook` 流 → BIFF8 记录 → 二维表。

use std::collections::{HashMap, HashSet};

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
const REC_FONT: u16 = 0x0031;
const REC_XF: u16 = 0x00E0;
const REC_PALETTE: u16 = 0x0092;

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

/// BIFF8 内置调色板（索引 0..=63），用来把字体颜色索引翻成 RGB。
///
/// 索引 >= 64 是「系统色」，原表里基本不会用；真遇到就当成说不清的颜色。
fn builtin_rgb(idx: u16) -> Option<(u8, u8, u8)> {
    const P: [(u8, u8, u8); 64] = [
    (0, 0, 0), (255, 255, 255), (255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0), (255, 0, 255), (0, 255, 255),
    (0, 0, 0), (255, 255, 255), (255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0), (255, 0, 255), (0, 255, 255),
    (128, 0, 0), (0, 128, 0), (0, 0, 128), (128, 128, 0), (128, 0, 128), (0, 128, 128), (192, 192, 192), (128, 128, 128),
    (153, 153, 255), (153, 51, 102), (255, 255, 204), (204, 255, 255), (102, 0, 102), (255, 128, 128), (0, 102, 204), (204, 204, 255),
    (0, 0, 128), (255, 0, 255), (255, 255, 0), (0, 255, 255), (128, 0, 128), (128, 0, 0), (0, 128, 128), (0, 0, 255),
    (0, 204, 255), (204, 255, 255), (204, 255, 204), (255, 255, 153), (153, 204, 255), (255, 153, 204), (204, 153, 255), (255, 204, 153),
    (51, 102, 255), (51, 204, 204), (153, 204, 0), (255, 204, 0), (255, 153, 0), (255, 102, 0), (102, 102, 153), (150, 150, 150),
    (0, 51, 102), (51, 153, 102), (0, 51, 0), (51, 51, 0), (153, 51, 0), (153, 51, 102), (51, 51, 153), (51, 51, 51),
    ];
    P.get(idx as usize).copied()
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
    /// 与 `rows` 同形状：该格字体是不是红色（原表里红色 = 已出货）。
    pub red: Vec<Vec<bool>>,
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
    /// 每个 XF 记录的字体索引（单元格记录里第 3 个 u16 就是 XF 索引）。
    xfs: Vec<u16>,
    /// 每个 FONT 记录的颜色，值是调色板索引。
    fonts: Vec<u16>,
    /// PALETTE 记录里的自定义颜色，覆盖内置调色板的 8..=63。
    palette: HashMap<u8, (u8, u8, u8)>,
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

        // 样式链：单元格 → XF → FONT → 调色板索引。只为了判「字体是不是红的」。
        let mut fonts: Vec<u16> = Vec::new();
        let mut xfs: Vec<u16> = Vec::new();
        let mut palette: HashMap<u8, (u8, u8, u8)> = HashMap::new();
        for (rid, body) in &recs {
            match *rid {
                REC_FONT => {
                    if body.len() >= 6 {
                        fonts.push(rd_u16(body, 4).unwrap_or(0));
                    }
                }
                REC_XF => {
                    if body.len() >= 2 {
                        xfs.push(rd_u16(body, 0).unwrap_or(0));
                    }
                }
                REC_PALETTE => {
                    let n = rd_u16(body, 0).unwrap_or(0) as usize;
                    for k in 0..n {
                        let p = 2 + k * 4;
                        if p + 2 < body.len() {
                            palette.insert((8 + k) as u8, (body[p], body[p + 1], body[p + 2]));
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(Book {
            wb,
            names,
            sst,
            xfs,
            fonts,
            palette,
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
        // 只记「是红的」那些格：原表里绝大多数格不是红的
        let mut red_cells: HashSet<(usize, usize)> = HashSet::new();
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
                        let xf = rd_u16(body, 4).unwrap_or(0) as usize;
                        let isst = rd_u32(body, 6).unwrap_or(0) as usize;
                        let text = self.sst.get(isst).cloned().unwrap_or_default();
                        cells.insert((row, col), text);
                        if self.is_red(xf) {
                            red_cells.insert((row, col));
                        }
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
                            if self.is_red(rd_u16(body, 4).unwrap_or(0) as usize) {
                                red_cells.insert((row, col));
                            }
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
                            if self.is_red(rd_u16(body, 4).unwrap_or(0) as usize) {
                                red_cells.insert((row, col));
                            }
                        }
                    }
                }
                REC_MULRK => {
                    if body.len() >= 6 {
                        let row = rd_u16(body, 0).unwrap_or(0) as usize;
                        let col0 = rd_u16(body, 2).unwrap_or(0) as usize;
                        let n = (body.len() - 6) / 6;
                        for k in 0..n {
                            let col = col0 + k;
                            if let Some(rk) = rd_u32(body, 4 + k * 6 + 2) {
                                cells.insert((row, col), fmt_num(rk_value(rk)));
                                if self.is_red(rd_u16(body, 4 + k * 6).unwrap_or(0) as usize) {
                                    red_cells.insert((row, col));
                                }
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
                            if self.is_red(rd_u16(body, 4).unwrap_or(0) as usize) {
                                red_cells.insert((row, col));
                            }
                        }
                    }
                }
                REC_FORMULA => {
                    if body.len() >= 20 {
                        if let (Some(row), Some(col)) = (
                            rd_u16(body, 0).map(usize::from),
                            rd_u16(body, 2).map(usize::from),
                        ) {
                            let red = self.is_red(rd_u16(body, 4).unwrap_or(0) as usize);
                            let res = &body[6..14];
                            if red {
                                red_cells.insert((row, col));
                            }
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
            let red = red_cells.contains(&(*r1, *c1));
            for row in *r1..=*r2 {
                for col in *c1..=*c2 {
                    cells.insert((row, col), v.clone());
                    if red {
                        red_cells.insert((row, col));
                    }
                }
            }
        }

        let max_row = cells.keys().map(|(r, _)| *r).max();
        let max_col = cells.keys().map(|(_, c)| *c).max();
        let mut rows = Vec::new();
        let mut red = Vec::new();
        if let (Some(max_row), Some(max_col)) = (max_row, max_col) {
            for r in 0..=max_row {
                let mut row = Vec::with_capacity(max_col + 1);
                let mut row_red = Vec::with_capacity(max_col + 1);
                for c in 0..=max_col {
                    row.push(cells.get(&(r, c)).cloned().unwrap_or_default());
                    row_red.push(red_cells.contains(&(r, c)));
                }
                rows.push(row);
                red.push(row_red);
            }
        }
        Ok(Sheet {
            name: real_name,
            rows,
            red,
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

    /// 某一格字体是不是红色。`xf` 是单元格记录里的 XF 索引。
    ///
    /// 判据放宽到「红得明显」：R ≥ 150 且 G、B ≤ 90。WPS 里手点的「红色」一般是
    /// 调色板 index 10 = FF0000，偶尔是 C00000 这类深红，都落在这个区间里；
    /// 洋红 / 珊瑚红（G 或 B 偏高）不算——那种在原表里另有含义。
    fn is_red(&self, xf: usize) -> bool {
        let Some(&font) = self.xfs.get(xf) else {
            return false;
        };
        // BIFF 保留字体编号 4，但文件中没有对应 FONT 记录。
        let font_record = match font {
            4 => return false,
            5.. => font as usize - 1,
            _ => font as usize,
        };
        let Some(&color) = self.fonts.get(font_record) else {
            return false;
        };
        // 调色板索引 >= 64 是系统色（原表基本不用），说不清就不算红
        let rgb = if color < 64 {
            self.palette
                .get(&(color as u8))
                .copied()
                .or_else(|| builtin_rgb(color))
        } else {
            None
        };
        matches!(rgb, Some((r, g, b)) if r >= 150 && g <= 90 && b <= 90)
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

#[cfg(test)]
mod font_index_tests {
    use super::*;
    #[test]
    fn biff_reserved_font_index_does_not_shift_red_detection() {
        let book = Book {
            wb: vec![], names: vec![], sst: vec![],
            xfs: vec![0, 4, 5, 6],
            fonts: vec![10, 8, 8, 8, 40, 10],
            palette: HashMap::new(),
        };
        assert!(book.is_red(0));
        assert!(!book.is_red(1), "保留编号不能读取下一种字体");
        assert!(!book.is_red(2), "编号 5 对应第 5 条字体记录，颜色 40 不是红色");
        assert!(book.is_red(3), "真正的红色仍须识别");
    }
}