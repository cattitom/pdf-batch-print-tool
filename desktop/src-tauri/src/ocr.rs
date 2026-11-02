//! 装箱单截图识别：两条引擎，产出同一份「词 + 坐标」。
//!
//! - **PaddleOCR（MNN 推理）**：`models/` 下有 `PP-OCRv6_medium_det.mnn`（最高精度档）或
//!   `PP-OCRv6_small_det.mnn` / `PP-OCRv5_mobile_det.mnn`（降级档）时优先用，
//!   中文/数字准确率明显高于系统 OCR；
//! - **Windows 自带 OCR**（`Windows.Media.Ocr`）：没有模型时的兜底，离线免费、零依赖。
//!
//! 两个引擎都只给「词 + 坐标」，不理解表格，所以后面统一补：按 y 聚成行、
//! 按 x 的空白槽切列，再按内容认角色（商品名 / 数量 / 标记 / 目的地 / 入库号），
//! 最后交给 [`pdfimpose_core::parse_grid`] 做校验、合并单元格展开和去重。

use std::future::Future;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::task::{Context, Poll, Wake, Waker};

use pdfimpose_core as core;
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::BitmapDecoder;
use windows::Media::Ocr::OcrEngine;
use windows::Storage::{FileAccessMode, StorageFile};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows_future::IAsyncOperation;

/// 一个识别出来的词及其外接框（像素）。
#[derive(Debug, Clone)]
pub struct Word {
    pub text: String,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Word {
    fn cx(&self) -> f32 {
        (self.x0 + self.x1) / 2.0
    }
    fn cy(&self) -> f32 {
        (self.y0 + self.y1) / 2.0
    }
    fn height(&self) -> f32 {
        self.y1 - self.y0
    }
}

// ---------- 阻塞等待 WinRT 异步操作 ----------
//
// windows-rs 只给 IAsyncOperation 实现 IntoFuture，没有 block_on。
// 这里用一个最小的 Parker（Mutex + Condvar）把 future 跑完。

struct Parker {
    ready: Mutex<bool>,
    cv: Condvar,
}

impl Wake for Parker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        *self.ready.lock().unwrap() = true;
        self.cv.notify_all();
    }
}

fn block_on<F: Future>(fut: F) -> F::Output {
    let parker = Arc::new(Parker { ready: Mutex::new(false), cv: Condvar::new() });
    let waker = Waker::from(parker.clone());
    let mut cx = Context::from_waker(&waker);
    let mut fut = pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        let mut ready = parker.ready.lock().unwrap();
        while !*ready {
            ready = parker.cv.wait(ready).unwrap();
        }
        *ready = false;
    }
}

fn wait<T>(op: IAsyncOperation<T>) -> Result<T, String>
where
    T: windows::core::RuntimeType + 'static,
{
    use std::future::IntoFuture as _;
    block_on(op.into_future()).map_err(|e| format!("Windows OCR 调用失败：{}", e.message()))
}

// ---------- 识别 ----------

/// 预处理：按给定倍数放大（小字直接 OCR 掉字严重）。
/// 试过灰度+二值化：黑边框会和字粘一起，反而更差，所以只做放大。
fn prepare_image(path: &str, scale: f32) -> Result<std::path::PathBuf, String> {
    let img = image::open(path).map_err(|e| format!("打不开图片：{e}"))?;
    let (w, h) = (img.width(), img.height());

    let out = if scale > 1.01 {
        img.resize(
            (w as f32 * scale) as u32,
            (h as f32 * scale) as u32,
            image::imageops::FilterType::CatmullRom,
        )
    } else {
        img
    };

    let tmp = std::env::temp_dir().join(format!(
        "pdfprint-ocr-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    out.save(&tmp).map_err(|e| format!("写临时图片失败：{e}"))?;
    if let Ok(debug) = std::env::var("OCR_DEBUG_PNG") {
        let _ = out.save(&debug);
    }
    Ok(tmp)
}

/// 放大倍数：目标宽度约 4800px，最多 3.2 倍。
fn scale_for(img_width: u32) -> f32 {
    let target = 4800.0f32;
    if (img_width as f32) < target {
        (target / img_width as f32).clamp(1.0, 3.2)
    } else {
        1.0
    }
}

/// 调用系统 OCR，拿回所有词。
///
/// 只跑一遍：试过「两个缩放比例各跑一次再按位置合并」，结果尾部会多出
/// 零碎字符（IN5LK113218I），反而更难用。
pub fn recognize_words(path: &str) -> Result<Vec<Word>, String> {
    let width = image::open(path)
        .map_err(|e| format!("打不开图片：{e}"))?
        .width();
    let tmp = prepare_image(path, scale_for(width))?;

    // COM 初始化放在独立线程：Tauri 的命令线程不一定初始化过 COM
    let tmp_for_thread = tmp.clone();
    let joined = std::thread::spawn(move || ocr_thread(&tmp_for_thread.to_string_lossy())).join();
    let _ = std::fs::remove_file(&tmp);

    match joined {
        Ok(r) => r,
        Err(_) => Err("OCR 线程异常退出".to_string()),
    }
}

fn ocr_thread(path: &str) -> Result<Vec<Word>, String> {
    // 已经初始化过（返回 RPC_E_CHANGED_MODE）也无所谓，继续用
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

    let result = run_ocr(path);
    unsafe { CoUninitialize() };
    result
}

fn run_ocr(path: &str) -> Result<Vec<Word>, String> {
    let file = wait(
        StorageFile::GetFileFromPathAsync(&HSTRING::from(path))
            .map_err(|e| format!("打开图片失败：{}", e.message()))?,
    )?;

    let stream = wait(
        file.OpenAsync(FileAccessMode::Read)
            .map_err(|e| format!("读取图片失败：{}", e.message()))?,
    )?;

    let decoder = wait(
        BitmapDecoder::CreateAsync(&stream).map_err(|e| format!("解码图片失败：{}", e.message()))?,
    )?;

    let bitmap = wait(
        decoder
            .GetSoftwareBitmapAsync()
            .map_err(|e| format!("转位图失败：{}", e.message()))?,
    )?;

    let engine = match Language::CreateLanguage(&HSTRING::from("zh-Hans-CN"))
        .map_err(|e| e.message())
        .and_then(|lang| {
            OcrEngine::TryCreateFromLanguage(&lang).map_err(|e| e.message())
        }) {
        Ok(engine) => engine,
        Err(_) => OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| {
            "这台机器没装中文 OCR 语言包（设置 → 时间和语言 → 语言 → 中文 → 可选功能）".to_string()
        })?,
    };

    let result = wait(
        engine
            .RecognizeAsync(&bitmap)
            .map_err(|e| format!("识别失败：{}", e.message()))?,
    )?;

    let lines = result.Lines().map_err(|e| e.message())?;
    let mut words = Vec::new();
    for i in 0..lines.Size().map_err(|e| e.message())? {
        let line = lines.GetAt(i).map_err(|e| e.message())?;
        let ws = line.Words().map_err(|e| e.message())?;
        for j in 0..ws.Size().map_err(|e| e.message())? {
            let w = ws.GetAt(j).map_err(|e| e.message())?;
            let text = w.Text().map_err(|e| e.message())?.to_string();
            let rect = w.BoundingRect().map_err(|e| e.message())?;
            if text.trim().is_empty() {
                continue;
            }
            words.push(Word {
                text: text.trim().to_string(),
                x0: rect.X,
                y0: rect.Y,
                x1: rect.X + rect.Width,
                y1: rect.Y + rect.Height,
            });
        }
    }
    Ok(words)
}

// ---------- 词 → 二维表 ----------

/// 按 y 聚类成行、按 x 的空白槽切成列。
pub fn words_to_grid(words: &[Word]) -> Vec<Vec<String>> {
    if words.is_empty() {
        return Vec::new();
    }
    let heights: Vec<f32> = words.iter().map(|w| w.height()).filter(|h| *h > 0.0).collect();
    let median_h = median(&heights).unwrap_or(10.0).max(4.0);

    // 行
    let mut by_y: Vec<&Word> = words.iter().collect();
    by_y.sort_by(|a, b| a.cy().partial_cmp(&b.cy()).unwrap_or(std::cmp::Ordering::Equal));
    let mut rows: Vec<Vec<&Word>> = Vec::new();
    let mut last_cy = f32::MIN;
    for w in by_y {
        if rows.is_empty() || w.cy() - last_cy > median_h * 0.25 {
            rows.push(vec![w]);
        } else if let Some(last) = rows.last_mut() {
            last.push(w);
        }
        last_cy = w.cy();
    }

    // 合并单元格里的字（比如标记列的「TK5 1」）会落在两行之间，自己成一行；
    // 把挨得过近的行并回去。
    loop {
        if rows.len() < 2 {
            break;
        }
        let centers: Vec<f32> = rows
            .iter()
            .map(|r| r.iter().map(|w| w.cy()).sum::<f32>() / r.len() as f32)
            .collect();
        let gaps: Vec<f32> = centers.windows(2).map(|w| w[1] - w[0]).collect();
        let med_gap = median(&gaps).unwrap_or(0.0);
        if med_gap <= 0.0 {
            break;
        }
        let mut merged = false;
        for i in 0..centers.len() - 1 {
            if centers[i + 1] - centers[i] < med_gap * 0.5 {
                let mut next = rows.remove(i + 1);
                if let Some(cur) = rows.get_mut(i) {
                    cur.append(&mut next);
                }
                merged = true;
                break;
            }
        }
        if !merged {
            break;
        }
    }

    // 列：整张表的 x 覆盖里，连续空白 >= gap 的地方就是列间隔
    let max_x = words.iter().map(|w| w.x1).fold(0.0f32, f32::max).ceil() as usize + 2;
    let mut covered = vec![false; max_x + 1];
    for w in words {
        let a = w.x0.floor().max(0.0) as usize;
        let b = w.x1.ceil() as usize;
        for slot in covered.iter_mut().take(b.min(max_x) + 1).skip(a) {
            *slot = true;
        }
    }
    let min_gap = (median_h * 0.6).max(4.0) as usize;
    let mut seps: Vec<f32> = Vec::new();
    let mut run_start: Option<usize> = None;
    for (x, hit) in covered.iter().enumerate() {
        match (hit, run_start) {
            (false, None) => run_start = Some(x),
            (true, Some(start)) => {
                if x - start >= min_gap && start > 0 {
                    seps.push(((start + x) as f32) / 2.0);
                }
                run_start = None;
            }
            _ => {}
        }
    }

    let col_of = |w: &Word| seps.iter().filter(|s| **s < w.cx()).count();

    // 组格子
    let mut rows_out: Vec<Vec<String>> = Vec::new();
    for row in rows {
        let mut cells: Vec<(usize, Vec<&Word>)> = Vec::new();
        let mut sorted = row.clone();
        sorted.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal));
        for w in sorted {
            let c = col_of(w);
            match cells.last_mut() {
                Some((idx, list)) if *idx == c => list.push(w),
                _ => cells.push((c, vec![w])),
            }
        }
        let mut out = vec![String::new(); cells.iter().map(|(c, _)| *c).max().map(|m| m + 1).unwrap_or(0)];
        for (c, list) in cells {
            let text = join_words(&list);
            if c < out.len() {
                out[c] = text;
            }
        }
        if !out.iter().all(|c| c.is_empty()) {
            rows_out.push(out);
        }
    }
    rows_out
}

// ---------- 按表格线切真实单元格 ----------
//
// 表格是黑线 + 彩色底：靠「近黑像素」就能找到列线和行线；
// 合并单元格 = 该列里两行之间没有横线，那格天然跨行，不用再猜。
// 切出来的网格是「合并格的值只落在它跨的第一行」，剩下的交给 core 的向下填充。

#[derive(Debug, Clone)]
struct Bands {
    cols: Vec<(f32, f32)>,
    /// 参考行（取行线最多的那一列）
    rows: Vec<(f32, f32)>,
    /// 每列自己的行线（用于判断合并）
    cells: Vec<Vec<(f32, f32)>>,
}

fn is_dark(p: image::Rgb<u8>) -> bool {
    p[0] < 90 && p[1] < 90 && p[2] < 90
}

/// 连续为 true 的区间，长度不足 min_len 的丢掉。
fn runs(flags: &[bool], min_len: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, &f) in flags.iter().enumerate() {
        match (f, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if i - s >= min_len {
                    out.push((s, i));
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        if flags.len() - s >= min_len {
            out.push((s, flags.len()));
        }
    }
    out
}

fn centers_of(rs: &[(usize, usize)]) -> Vec<f32> {
    rs.iter().map(|(s, e)| (*s as f32 + *e as f32) / 2.0).collect()
}

fn bands_between(seps: &[f32], lo: f32, hi: f32, min_size: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut prev = lo;
    for s in seps {
        if *s - prev >= min_size {
            out.push((prev, *s));
        }
        prev = *s;
    }
    if hi - prev >= min_size {
        out.push((prev, hi));
    }
    out
}

fn detect_bands(path: &str) -> Option<Bands> {
    let img = image::open(path).ok()?.to_rgb8();
    let (w, h) = img.dimensions();
    if w < 300 || h < 120 {
        return None;
    }

    // 列线：整列近黑像素比例高
    let mut col_flags = vec![false; w as usize];
    for x in 0..w {
        let mut n = 0u32;
        for y in 0..h {
            if is_dark(*img.get_pixel(x, y)) {
                n += 1;
            }
        }
        col_flags[x as usize] = n as f32 / h as f32 > 0.5;
    }
    let vcenters = centers_of(&runs(&col_flags, 1));
    if vcenters.len() < 4 {
        return None;
    }
    let cols = bands_between(&vcenters, 0.0, w as f32, 24.0);
    if cols.len() < 5 {
        return None;
    }

    // 每列各自的行线
    let mut cells: Vec<Vec<(f32, f32)>> = Vec::new();
    for (x0, x1) in &cols {
        let a = x0.max(0.0) as u32 + 2;
        let b = (*x1 as u32).min(w).saturating_sub(2);
        if b <= a {
            cells.push(Vec::new());
            continue;
        }
        let mut row_flags = vec![false; h as usize];
        for y in 0..h {
            let mut n = 0u32;
            for x in a..b {
                if is_dark(*img.get_pixel(x, y)) {
                    n += 1;
                }
            }
            row_flags[y as usize] = n as f32 / (b - a) as f32 > 0.5;
        }
        let seps = centers_of(&runs(&row_flags, 1));
        cells.push(bands_between(&seps, 0.0, h as f32, 10.0));
    }

    // 行线最多的那一列当参考行
    let ref_col = cells.iter().enumerate().max_by_key(|(_, v)| v.len())?.0;
    let rows = cells[ref_col].clone();
    if rows.len() < 3 {
        return None;
    }

    Some(Bands { cols, rows, cells })
}

/// 用表格线切出来的格子重组网格：同一格里的词按 x 排序拼接，
/// 合并格只写进它覆盖的第一行（后续行留空，交给 core 向下填充）。
fn grid_from_bands(bands: &Bands, words: &[Word]) -> Vec<Vec<String>> {
    let mut grid: Vec<Vec<String>> = vec![vec![String::new(); bands.cols.len()]; bands.rows.len()];

    for (ci, (x0, x1)) in bands.cols.iter().enumerate() {
        let col_cells = &bands.cells[ci];
        for (y0, y1) in col_cells {
            let mut inside: Vec<&Word> = words
                .iter()
                .filter(|w| w.cx() >= *x0 && w.cx() < *x1 && w.cy() >= *y0 && w.cy() < *y1)
                .collect();
            if inside.is_empty() {
                continue;
            }
            inside.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal));
            let text = join_words(&inside);
            let ri = bands
                .rows
                .iter()
                .position(|(ry0, ry1)| ry1 > y0 && ry0 < y1)
                .unwrap_or(0);
            if ri < grid.len() {
                grid[ri][ci] = text;
            }
        }
    }

    grid.retain(|row| row.iter().any(|c| !c.is_empty()));
    grid
}

/// 中文被 OCR 拆成单字，拼回去时不加空格；数字里的小数点、斜杠同理。
fn join_words(list: &[&Word]) -> String {
    let mut out = String::new();
    for w in list {
        let t = w.text.as_str();
        if out.is_empty() {
            out.push_str(t);
            continue;
        }
        let prev = out.chars().last().unwrap_or(' ');
        let next = t.chars().next().unwrap_or(' ');
        if !no_space(prev, next) {
            out.push(' ');
        }
        out.push_str(t);
    }
    out.replace('．', ".")
        .replace('，', ",")
        .replace('：', ":")
        .replace('（', "(")
        .replace('）', ")")
}

fn no_space(prev: char, next: char) -> bool {
    is_wide(prev) || is_wide(next) || is_punct(prev) || is_punct(next) || prev == '/' || next == '/'
}

fn is_wide(c: char) -> bool {
    let u = c as u32;
    (0x3000..=0x9FFF).contains(&u) || (0xFF00..=0xFFEF).contains(&u)
}

fn is_punct(c: char) -> bool {
    matches!(
        c,
        '.' | ',' | ':' | ';' | '(' | ')' | '-' | '、' | '。' | '．' | '，' | '：'
    )
}

/// OCR 常把入库号开头的 I 认成 丨 / l / 1，修回来。
fn fix_id(s: &str) -> String {
    let mut t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    t = t.replace('丨', "I").replace('|', "I").replace('｜', "I");
    let mut chars = t.chars();
    if let (Some(first), Some(second)) = (chars.next(), chars.next()) {
        let bad_i = matches!(first, 'l' | '1') || first == 'I';
        if bad_i && second.is_ascii_uppercase() {
            let rest: String = t.chars().skip(1).collect();
            t = format!("I{rest}");
        }
    }
    t
}

fn median(values: &[f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(v[v.len() / 2])
}

// ---------- 表格 → 11 列（core 认得的模板） ----------

#[derive(Debug, Clone, Copy)]
struct Roles {
    name: usize,
    qty: usize,
    inbound: usize,
    dest: Option<usize>,
    mark: Option<usize>,
}


/// 单元格里是否有独立的「1」（`1`、`TK5 1` 都算）。
fn has_standalone_one(cell: &str) -> bool {
    let t = cell.trim();
    t == "1" || t.split_whitespace().any(|x| x == "1") || t.split(|c: char| !c.is_alphanumeric()).any(|x| x == "1")
}
fn is_int(cell: &str) -> Option<i64> {
    let t = cell.trim().replace(',', "");
    if t.is_empty() || t.len() > 7 || !t.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    t.parse::<i64>().ok()
}

fn is_id_like(cell: &str) -> bool {
    let t = cell.trim();
    if t.len() < 5 || t.len() > 24 || t.contains(char::is_whitespace) {
        return false;
    }
    if !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return false;
    }
    let letters = t.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    letters >= 1 && digits >= 2
}

/// 从整张表推断每列是什么。
fn detect_roles(grid: &[Vec<String>]) -> Option<Roles> {
    let ncols = grid.iter().map(|r| r.len()).max().unwrap_or(0);
    if ncols == 0 || grid.len() < 2 {
        return None;
    }
    let col_values = |c: usize| -> Vec<String> {
        grid.iter()
            .filter_map(|r| r.get(c))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };

    // 入库号列：字母+数字混合的最多
    let inbound = (0..ncols)
        .map(|c| (c, col_values(c).iter().filter(|v| is_id_like(&fix_id(v))).count()))
        .max_by_key(|(_, n)| *n)?;
    if inbound.1 == 0 {
        return None;
    }

    // 数量列：入库号左边，整数、最大值 >= 3
    let mut qty: Option<(usize, i64)> = None;
    for c in 0..inbound.0 {
        let vals: Vec<i64> = col_values(c).iter().filter_map(|v| is_int(v)).collect();
        if vals.is_empty() {
            continue;
        }
        let nonempty = col_values(c).len();
        if vals.len() * 10 < nonempty * 6 {
            continue;
        }
        let max = *vals.iter().max().unwrap_or(&0);
        if max >= 3 {
            qty = Some((c, max));
        }
    }
    let (qty, _) = qty?;

    // 标记列：数量右边，非空格子基本只有 1
    let mut mark = None;
    for c in (qty + 1)..inbound.0 {
        let vals = col_values(c);
        if vals.is_empty() {
            continue;
        }
        let ones = vals.iter().filter(|v| has_standalone_one(v)).count();
        if ones * 10 >= vals.len() * 6 {
            mark = Some(c);
            break;
        }
    }

    // 目的地：标记列右边、入库号左边，短文本且重复
    let mut dest = None;
    let mut dest_best = 0usize;
    for c in (mark.unwrap_or(qty) + 1)..inbound.0 {
        let vals = col_values(c);
        if vals.len() < 2 {
            continue;
        }
        let short = vals.iter().filter(|v| v.chars().count() <= 8 && is_int(v).is_none()).count();
        if short * 10 < vals.len() * 8 {
            continue;
        }
        if vals.iter().any(|v| is_id_like(v)) {
            continue;
        }
        if c > dest_best {
            dest_best = c;
            dest = Some(c);
        }
    }

    // 商品名：数量左边、平均字数最长
    let mut name = 0usize;
    let mut best_len = 0f32;
    for c in 0..qty.max(1) {
        let vals = col_values(c);
        if vals.is_empty() {
            continue;
        }
        let avg = vals.iter().map(|v| v.chars().count() as f32).sum::<f32>() / vals.len() as f32;
        if avg > best_len {
            best_len = avg;
            name = c;
        }
    }

    Some(Roles { name, qty, inbound: inbound.0, dest, mark })
}

/// 把任意切法的表格压成 core 认的 11 列模板，并把单号/合计这类散行附在后面。
fn to_canonical(grid: &[Vec<String>]) -> Vec<Vec<String>> {
    let roles = match detect_roles(grid) {
        Some(r) => r,
        None => return grid.to_vec(),
    };
    let get = |row: &Vec<String>, c: Option<usize>| -> String {
        c.and_then(|c| row.get(c)).map(|s| s.trim().to_string()).unwrap_or_default()
    };

    let mut out: Vec<Vec<String>> = Vec::new();
    for row in grid {
        let name = get(row, Some(roles.name));
        let qty = get(row, Some(roles.qty));
        let inbound = fix_id(&get(row, Some(roles.inbound)));
        let dest = get(row, roles.dest);
        let mark = get(row, roles.mark);
        if name.is_empty() && qty.is_empty() && inbound.is_empty() {
            continue;
        }
        out.push(vec![
            name,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            qty,
            mark,
            String::new(),
            dest,
            inbound,
        ]);
    }

    // 单号、合计写在表格右侧，作为独立行交给 core 去扫
    for row in grid {
        for cell in row {
            let t = cell.trim();
            if t.contains("单号") || t.contains('现') {
                out.push(vec![t.to_string()]);
            }
        }
    }
    out
}

// ---------- PaddleOCR（可选：有 .mnn 模型时优先） ----------

/// 模型目录，按优先级找：
/// 1. exe 同级 `models/`（打包后把模型拷到 exe 旁边）
/// 2. exe 同级 `resources/models/`（Tauri 打包资源目录）
/// 3. 开发时的 `desktop/models/`
fn models_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("models"));
            candidates.push(dir.join("resources").join("models"));
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("models"));
    candidates
        .into_iter()
        .find(|dir| !available_sets(dir).is_empty())
}

/// `models/` 里实际存在、文件齐全的模型档，按精度从高到低：
/// 1. v6 medium（官方 accuracy-first 档，最准）
/// 2. v6 small（均衡档）
/// 3. v5 mobile（老轻量档，兜底）
/// 想换档位时，把三个文件丢进 `models/` 即可，代码不用改。
fn available_sets(dir: &std::path::Path) -> Vec<(PathBuf, PathBuf, PathBuf, &'static str)> {
    const SETS: &[(&str, &str, &str, &str)] = &[
        // 检测用 medium（框更稳），识别用 small（窄列里两个「1」并排时不容易漏），
        // 实测三张真截图全对；纯 medium 会在第三张漏掉一个「1」。
        (
            "PP-OCRv6_medium_det.mnn",
            "PP-OCRv6_small_rec.mnn",
            "ppocr_keys_v6_small.txt",
            "PP-OCRv6-medium-det+small-rec",
        ),
        (
            "PP-OCRv6_medium_det.mnn",
            "PP-OCRv6_medium_rec.mnn",
            "ppocr_keys_v6_medium.txt",
            "PP-OCRv6-medium",
        ),
        (
            "PP-OCRv6_small_det.mnn",
            "PP-OCRv6_small_rec.mnn",
            "ppocr_keys_v6_small.txt",
            "PP-OCRv6-small",
        ),
        (
            "PP-OCRv5_mobile_det.mnn",
            "PP-OCRv5_mobile_rec.mnn",
            "ppocr_keys_v5.txt",
            "PP-OCRv5-mobile",
        ),
    ];
    SETS.iter()
        .filter_map(|(det, rec, dict, name)| {
            let (d, r, k) = (dir.join(det), dir.join(rec), dir.join(dict));
            (d.exists() && r.exists() && k.exists()).then_some((d, r, k, *name))
        })
        .collect()
}

/// 可用档位名，精度从高到低。
fn available_tiers() -> Vec<&'static str> {
    models_dir()
        .map(|dir| {
            available_sets(&dir)
                .into_iter()
                .map(|(_, _, _, n)| n)
                .collect()
        })
        .unwrap_or_default()
}

/// 引擎初始化要读几十 MB 模型，进程内每档只做一次。
fn engine_for(tier: &'static str) -> Option<Arc<ocr_rs::OcrEngine>> {
    static CACHE: OnceLock<Mutex<Vec<(&'static str, Option<Arc<ocr_rs::OcrEngine>>)>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, engine)) = guard.iter().find(|(name, _)| *name == tier) {
        return engine.clone();
    }
    let engine = build_engine(tier);
    guard.push((tier, engine.clone()));
    engine
}

fn build_engine(tier: &'static str) -> Option<Arc<ocr_rs::OcrEngine>> {
    let dir = models_dir()?;
    let (det, rec, dict, name) = available_sets(&dir)
        .into_iter()
        .find(|(_, _, _, n)| *n == tier)?;
    let started = std::time::Instant::now();
    // 默认配置会丢掉低置信度的单个数字（标记列的「1」），这里放宽；
    // max_side_len 默认 960 会把 1600+ 宽的截图缩小一半，小字直接糊掉，必须调大。
    let mut cfg = ocr_rs::OcrEngineConfig::default();
    cfg.min_result_confidence = 0.2;
    // 最高精度档：MNN 高精度推理（不走低精度加速），线程按逻辑核拉满
    cfg.precision_mode = ocr_rs::PrecisionMode::High;
    cfg.thread_count = std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4)
        .clamp(4, 20);
    cfg.det_options.max_side_len = 4096;
    cfg.det_options.box_threshold = 0.4;
    cfg.det_options.score_threshold = 0.25;
    cfg.det_options.min_area = 4;
    cfg.det_options.merge_boxes = false;
    match ocr_rs::OcrEngine::new(
        det.to_str()?,
        rec.to_str()?,
        dict.to_str()?,
        Some(cfg),
    ) {
        Ok(engine) => {
            log::info!(
                "PaddleOCR 模型 {name} 加载完成（{}ms）",
                started.elapsed().as_millis()
            );
            Some(Arc::new(engine))
        }
        Err(e) => {
            log::warn!("PaddleOCR 模型 {name} 初始化失败：{e}");
            None
        }
    }
}

/// PaddleOCR：小图先放大再识别。
/// 这类截图一行才 26px 高、单个数字才 10px，检测阶段很容易整格漏掉，
/// 放大 2 倍后单个「1」才稳。
fn recognize_words_paddle_with(
    path: &str,
    engine: &ocr_rs::OcrEngine,
) -> Result<Vec<Word>, String> {
    let mut img = image::open(path).map_err(|e| format!("打不开图片：{e}"))?;
    let mut scale = 1.0f32;
    if img.width() < 2400 {
        let (w, h) = (img.width() * 2, img.height() * 2);
        img = img.resize_exact(w, h, image::imageops::FilterType::CatmullRom);
        scale = 2.0;
    }
    let items = engine
        .recognize(&img)
        .map_err(|e| format!("PaddleOCR 识别失败：{e}"))?;

    Ok(items
        .into_iter()
        .filter(|it| !it.text.trim().is_empty())
        .map(|it| {
            let r = it.bbox.rect;
            Word {
                text: it.text.trim().to_string(),
                x0: r.left() as f32 / scale,
                y0: r.top() as f32 / scale,
                x1: r.right() as f32 / scale,
                y1: r.bottom() as f32 / scale,
            }
        })
        .collect())
}

/// 当前会用哪个引擎：`"paddle"` / `"system"`。
pub fn engine_name() -> &'static str {
    if available_tiers().is_empty() {
        "system"
    } else {
        "paddle"
    }
}

/// 通用「图片 → 词」：有 Paddle 模型就用（精度高），没有或失败退回系统 OCR。
/// 给产品标签照片这类非表格图片用。
pub fn recognize_text_words(path: &str) -> Result<Vec<Word>, String> {
    for tier in available_tiers() {
        if let Some(engine) = engine_for(tier) {
            match recognize_words_paddle_with(path, &engine) {
                Ok(words) if !words.is_empty() => return Ok(words),
                Ok(_) => {}
                Err(e) => log::warn!("PaddleOCR（{tier}）识别失败：{e}"),
            }
        }
    }
    recognize_words(path)
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}')
}

/// 像产品编码的串：大写字母/数字/连字符，首尾是字母或数字，至少两个字母、一个连字符。
/// 例：`CX-GG-60PCS`、`ZX-WYB-LF`。用来从标签照片里挑编码，避免把名称或条码噪声当编码。
fn looks_like_code(s: &str) -> bool {
    let t = s.trim();
    if t.len() < 5 || t.len() > 32 {
        return false;
    }
    if !t.starts_with(|c: char| c.is_ascii_uppercase() || c.is_ascii_digit()) {
        return false;
    }
    if !t.ends_with(|c: char| c.is_ascii_uppercase() || c.is_ascii_digit()) {
        return false;
    }
    let (mut letters, mut digits, mut hyphens) = (0, 0, 0);
    for c in t.chars() {
        if c.is_ascii_uppercase() {
            letters += 1;
        } else if c.is_ascii_digit() {
            digits += 1;
        } else if c == '-' {
            hyphens += 1;
        } else {
            return false;
        }
    }
    letters >= 2 && hyphens >= 1 && (digits >= 1 || hyphens >= 2)
}

/// 产品标签照片 → (编码, 名称)。
///
/// 标签上编码一般出现两次（条码下方小字 + 大字），名称是中文行，
/// 照片里还有条码、气泡袋这些干扰，所以不切表格：
/// 编码取「出现次数最多、字最大」的那串，名称取所有含汉字的词按位置拼起来。
/// 一张图当一条记录；多条就选多张图。
pub fn parse_label_image(path: &str) -> Result<(String, String), String> {
    let words = recognize_text_words(path)?;
    if words.is_empty() {
        return Err("图片里没识别出文字，换张清楚点的照片".to_string());
    }

    let mut tallies: Vec<(String, usize, f32)> = Vec::new();
    for w in &words {
        let up = w.text.trim().to_ascii_uppercase();
        if !looks_like_code(&up) {
            continue;
        }
        match tallies.iter_mut().find(|(code, _, _)| *code == up) {
            Some(slot) => {
                slot.1 += 1;
                slot.2 = slot.2.max(w.height());
            }
            None => tallies.push((up, 1, w.height())),
        }
    }
    let code = tallies
        .into_iter()
        // 先看出现次数，再看字高（大字比条码下的小字可靠）
        .max_by(|a, b| {
            a.1.cmp(&b.1)
                .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
        })
        .map(|(code, _, _)| code)
        .ok_or_else(|| "没认出编码：正对标签、避开反光再试".to_string())?;

    let mut name_words: Vec<&Word> = words
        .iter()
        .filter(|w| w.text.chars().any(is_cjk))
        .collect();
    // 同一行的词 y 会差几个像素，先按 y 聚行再按 x 排，
    // 否则「-60个装」可能因为 y 小一点跑到「挂钩」前面
    let line_h = median(&words.iter().map(|w| w.height()).collect::<Vec<_>>()).unwrap_or(10.0);
    name_words.sort_by(|a, b| {
        a.cy()
            .partial_cmp(&b.cy())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut name_rows: Vec<Vec<&Word>> = Vec::new();
    for w in name_words {
        let same_line = name_rows
            .last()
            .map(|row| {
                let cy = row.iter().map(|x| x.cy()).sum::<f32>() / row.len() as f32;
                (w.cy() - cy).abs() <= line_h * 0.6
            })
            .unwrap_or(false);
        if same_line {
            if let Some(row) = name_rows.last_mut() {
                row.push(w);
            }
        } else {
            name_rows.push(vec![w]);
        }
    }
    for row in &mut name_rows {
        row.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal));
    }
    let name = name_rows
        .iter()
        .flat_map(|row| row.iter())
        .map(|w| w.text.trim())
        .collect::<String>();

    Ok((code, name))
}

/// 合计校验：表里写了「合计 N 件」就对得上，没写（None）就算过。
fn totals_match(sheet: &core::PackingSheet) -> bool {
    sheet
        .expected_qty
        .map_or(true, |expected| expected == sheet.total_qty)
}

// ---------- 对外入口 ----------

/// 图片 → 待办清单。整条链路：OCR（Paddle 优先，系统兜底）→ 切行切列 → 认列 → core 解析。
///
/// 有多档模型时先用最高精度档（v6 medium）；合计对不上时换下一档重跑，
/// 取能对上合计的那份。medium 在个别小字上会漏字符，单跑一档反而不如这样稳。
pub fn parse_image(path: &str) -> Result<core::PackingSheet, String> {
    let tiers = available_tiers();
    if tiers.is_empty() {
        let words = recognize_words(path)?;
        return build_sheet(path, words);
    }

    let mut fallback: Option<core::PackingSheet> = None;
    for tier in &tiers {
        let engine = match engine_for(tier) {
            Some(e) => e,
            None => continue,
        };
        let words = match recognize_words_paddle_with(path, &engine) {
            Ok(w) if !w.is_empty() => w,
            Ok(_) => continue,
            Err(e) => {
                log::warn!("档位 {tier} 识别失败：{e}");
                continue;
            }
        };
        let sheet = build_sheet(path, words)?;
        log::info!(
            "OCR 档位 {tier}：{} 行，合计 {}（表里写 {:?}）",
            sheet.items.len(),
            sheet.total_qty,
            sheet.expected_qty
        );
        if totals_match(&sheet) {
            return Ok(sheet);
        }
        if fallback.is_none() {
            fallback = Some(sheet);
        }
    }

    if let Some(sheet) = fallback {
        return Ok(sheet);
    }
    log::warn!("PaddleOCR 各档位都不可用，回退系统 OCR");
    let words = recognize_words(path)?;
    build_sheet(path, words)
}

/// 识别出的词 → 切行切列 → 认列 → core 解析。
fn build_sheet(path: &str, words: Vec<Word>) -> Result<core::PackingSheet, String> {
    if words.is_empty() {
        return Err("图片里没识别出文字，换张清晰点的图试试".to_string());
    }
    // 优先按表格黑线切真实单元格（合并单元格才不会认错行）；切不出来再退回按坐标聚类
    let grid = match detect_bands(path) {
        Some(bands) if bands.rows.len() >= 3 => {
            let g = grid_from_bands(&bands, &words);
            if g.len() >= 3 {
                log::info!(
                    "OCR 按表格线切格：{} 列 × {} 行（每列行线数：{:?}）",
                    bands.cols.len(),
                    bands.rows.len(),
                    bands.cells.iter().map(|c| c.len()).collect::<Vec<_>>()
                );
                g
            } else {
                words_to_grid(&words)
            }
        }
        _ => words_to_grid(&words),
    };
    // 出问题时把这两行日志发我就能定位（%APPDATA%\com.hanlin.pdfprint\logs\app.log）
    log::info!("OCR 原始网格 {} 行：{grid:?}", grid.len());
    let canonical = to_canonical(&grid);
    Ok(core::parse_grid(canonical))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("core")
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    #[test]
    fn grid_from_words_groups_rows_and_columns() {
        // 假造 3 行 3 列
        let mut words = Vec::new();
        let texts = [
            ["A1", "B1", "C1"],
            ["A2", "B2", "C2"],
            ["A3", "B3", "C3"],
        ];
        for (r, row) in texts.iter().enumerate() {
            for (c, t) in row.iter().enumerate() {
                words.push(Word {
                    text: t.to_string(),
                    x0: c as f32 * 100.0,
                    x1: c as f32 * 100.0 + 40.0,
                    y0: r as f32 * 30.0,
                    y1: r as f32 * 30.0 + 12.0,
                });
            }
        }
        let grid = words_to_grid(&words);
        assert_eq!(grid.len(), 3);
        assert_eq!(grid[1][1], "B2");
    }

    /// 真截图跑一遍（12 行、168 件、单号 6812704625）。
    ///
    /// 装了 PaddleOCR 模型时要求**全对**；只有系统 OCR 时放宽（它会读丢格子），
    /// 那种情况靠界面标黄 + 人工确认兜底。
    #[test]
    fn ocr_reads_sample_screenshot() {
        let p = fixture("装箱单样例.png");
        if !p.exists() {
            return;
        }
        let sheet = match parse_image(p.to_str().unwrap()) {
            Ok(s) => s,
            Err(e) => panic!("OCR 解析失败：{e}"),
        };
        let dump: Vec<String> = sheet
            .items
            .iter()
            .map(|i| format!("{}|{}|{}|{}", i.name, i.qty, i.inbound, i.checked))
            .collect();
        println!("引擎={} 行数={} 合计={} 单号={} 告警={:?}", engine_name(), sheet.items.len(), sheet.total_qty, sheet.waybill, sheet.warnings);
        for d in &dump {
            println!("  {d}");
        }

        assert_eq!(sheet.waybill, "6812704625", "单号要能从右侧抓出来");

        if engine_name() == "paddle" {
            assert_eq!(sheet.items.len(), 12, "行数不对：{:?}", dump);
            assert_eq!(sheet.total_qty, 168, "合计不对：{:?}", dump);
            assert!(sheet.warnings.is_empty(), "不该有告警：{:?}", sheet.warnings);
            assert!(
                sheet.items.iter().all(|i| !i.name.is_empty() && i.qty > 0),
                "有行没读到名称或数量：{:?}",
                dump
            );
        } else {
            assert!(sheet.items.len() >= 11, "识别行数太少：{:?}", dump);
            let with_qty = sheet.items.iter().filter(|i| i.qty > 0).count();
            assert!(with_qty >= 6, "数量识别率太低（{with_qty}）：{:?}", dump);
            assert!(
                sheet.total_qty != 168 || !sheet.warnings.is_empty(),
                "合计不对却没有告警：{:?}",
                dump
            );
        }
    }

    /// 第二张真截图（SAM美东 / 6812704706 / 7 行 / 合计 66 件）。
    /// 它的坑：入库号 IB058261023RT 跨 3 行、数量列里有几个单独的「1」。
    #[test]
    fn ocr_reads_second_sample() {
        let p = fixture("装箱单样例2.png");
        if !p.exists() {
            return;
        }
        let sheet = match parse_image(p.to_str().unwrap()) {
            Ok(s) => s,
            Err(e) => panic!("OCR 解析失败：{e}"),
        };
        let dump: Vec<String> = sheet
            .items
            .iter()
            .map(|i| format!("{}|{}|{}|{}|{:?}", i.name, i.qty, i.inbound, i.checked, i.issues))
            .collect();
        println!("引擎={} 行数={} 合计={} 单号={} 告警={:?}", engine_name(), sheet.items.len(), sheet.total_qty, sheet.waybill, sheet.warnings);
        for d in &dump {
            println!("  {d}");
        }

        assert_eq!(sheet.waybill, "6812704706");
        assert_eq!(sheet.items.len(), 7, "行数不对：{:?}", dump);
        assert_eq!(sheet.total_qty, 66, "合计应是 66 件：{:?}", dump);
        assert!(
            sheet.items.iter().all(|i| !i.inbound.is_empty()),
            "合并单元格的入库号要铺满 7 行：{:?}",
            dump
        );
    }

    /// 第三张真截图（SAM美东 / 6812704651 / 一大批合并单元格）。
    #[test]
    fn ocr_reads_third_sample() {
        let p = fixture("装箱单样例3.png");
        if !p.exists() {
            return;
        }
        let sheet = match parse_image(p.to_str().unwrap()) {
            Ok(s) => s,
            Err(e) => panic!("OCR 解析失败：{e}"),
        };
        let dump: Vec<String> = sheet
            .items
            .iter()
            .map(|i| format!("{}|{}|{}|{}|{:?}", i.name, i.qty, i.inbound, i.checked, i.issues))
            .collect();
        println!("引擎={} 行数={} 合计={} 单号={} 目的地={:?} 告警={:?}", engine_name(), sheet.items.len(), sheet.total_qty, sheet.waybill, sheet.destination, sheet.warnings);
        for d in &dump {
            println!("  {d}");
        }

        assert_eq!(sheet.waybill, "6812704651");
        assert!(sheet.items.len() >= 21, "行数太少：{:?}", dump);
        assert_eq!(sheet.total_qty, 128, "全部 22 行合计应是 128 件：{:?}", dump);
        assert!(
            sheet.items.iter().filter(|i| i.qty > 0).count() >= 21,
            "有行的数量没读到：{:?}",
            dump
        );
        // 第一行是从别的表裁进来的（背景色不同，用户会手动删），
        // 它以外的行目的地都应是 SAM美东
        let sam = sheet.items.iter().filter(|i| i.destination == "SAM美东").count();
        assert!(sam >= 20, "目的地识别不对（{sam} 行是 SAM美东）：{:?}", dump);
        assert!(
            sheet.items.iter().all(|i| !i.inbound.is_empty()),
            "合并单元格的入库号要铺满：{:?}",
            dump
        );
    }

    #[test]
    fn debug_dump() {
        let p = fixture("装箱单样例.png");
        if !p.exists() { return; }
        let Some(tier) = available_tiers().first().copied() else { return };
        let Some(engine) = engine_for(tier) else { return };
        let words = recognize_words_paddle_with(p.to_str().unwrap(), &engine).unwrap();
        println!("tier={tier} words={}", words.len());
        for w in &words { println!("({:.0},{:.0})-({:.0},{:.0}) {}", w.x0, w.y0, w.x1, w.y1, w.text); }
        let grid = words_to_grid(&words);
        for (i, r) in grid.iter().enumerate() { println!("{i:>2} | {}", r.join(" ‖ ")); }
    }
}
