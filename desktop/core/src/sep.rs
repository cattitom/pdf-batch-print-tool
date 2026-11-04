//! 分隔页：一行居中文字的**矢量路径**页。
//!
//! 旧实现用 pdfium 把整份 CJK 字体（约 20MB）嵌进 PDF，再 300dpi
//! 扁平化成灰度位图。位图有分辨率上限，放大/打印一定糊，文件还有几百 KB。
//!
//! 现在直接用 `ttf-parser` 取字形轮廓，把 TrueType 的二次贝塞尔转成
//! PDF 的三次贝塞尔，用 `m` / `l` / `c` / `h` / `f` 算子画进内容流：
//! - 一个字体字节都不进 PDF（没有 Font / FontFile 对象），文件只有几 KB；
//! - 纯矢量，任意缩放、任意 DPI 打印都锐利；
//! - 仍然按 `(宽, 高, 文字)` 缓存生成结果（见 [`separator_pdf`]），
//!   一次拼版里 N 个文件只付一次解析/生成代价。
//!
//! 坐标约定：TrueType 是 font units、y 向上，PDF 也是 y 向上，直接按
//! `font_size / units_per_em` 放大即可，无需翻转。所有字形共用一个
//! 非零环绕规则的 `f`（TrueType 轮廓本身就是按非零环绕定义的）。

use crate::error::ImposeError;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock};

use lopdf::{dictionary, Dictionary, Document, Object, Stream};

use crate::font;

/// 字号（pt）：与旧实现保持一致。
const FONT_SIZE: f64 = 28.0;
/// 文字填充色（#333333）。
const TEXT_GRAY: f64 = 0.2;

type Key = (i64, i64, String);

static CACHE: OnceLock<Mutex<HashMap<Key, Arc<Vec<u8>>>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<Key, Arc<Vec<u8>>>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 取一张指定尺寸的分隔页 PDF（缓存命中则零成本）。尺寸单位 pt。
pub fn separator_pdf(
    w: f64,
    h: f64,
    text: &str,
) -> Result<Arc<Vec<u8>>, ImposeError> {
    // 0.01pt 精度做键，避免浮点尾差导致缓存穿透
    let key = ((w * 100.0).round() as i64, (h * 100.0).round() as i64, text.to_string());
    let guard = cache().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(hit) = guard.get(&key) {
        return Ok(hit.clone());
    }
    drop(guard);
    let bytes = Arc::new(render(w, h, text)?);
    cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, bytes.clone());
    Ok(bytes)
}

/// 实际生成：手搓单页 PDF（白底 + 矢量文字），不经过 pdfium、不栅格化。
fn render(w: f64, h: f64, text: &str) -> Result<Vec<u8>, ImposeError> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();

    // 内容流：压缩后通常只有几 KB
    let mut content = Stream::new(Dictionary::new(), build_content(w, h, text));
    content.compress()?;
    let content_id = doc.add_object(content);

    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Real(w as f32),
            Object::Real(h as f32),
        ],
        "Resources" => Dictionary::new(),
        "Contents" => content_id,
    });

    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => Object::Integer(1),
        }),
    );

    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);

    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)?;
    Ok(bytes)
}

/// 一条 PDF 路径指令（坐标已是 pt，相对排版原点）。
#[derive(Clone, Copy)]
enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Curve(f64, f64, f64, f64, f64, f64),
    Close,
}

/// 把 ttf-parser 的轮廓回调转成 PDF 路径指令。
///
/// `pen_x` 是当前字形的起始 x（pt），`scale` 是 font units → pt。
/// PDF 没有二次贝塞尔：`quad_to` 按
/// `C1 = P0 + 2/3(Q-P0)`、`C2 = P1 + 2/3(Q-P1)` 精确转成三次。
struct OutlineSink<'a> {
    segs: &'a mut Vec<Seg>,
    pen_x: f64,
    scale: f64,
    cur: (f64, f64),
    start: (f64, f64),
}

impl OutlineSink<'_> {
    fn pt(&self, x: f32, y: f32) -> (f64, f64) {
        (self.pen_x + x as f64 * self.scale, y as f64 * self.scale)
    }
}

impl ttf_parser::OutlineBuilder for OutlineSink<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.pt(x, y);
        self.segs.push(Seg::Move(p.0, p.1));
        self.cur = p;
        self.start = p;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.pt(x, y);
        self.segs.push(Seg::Line(p.0, p.1));
        self.cur = p;
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let control = self.pt(x1, y1);
        let end = self.pt(x, y);
        let p0 = self.cur;
        let c1 = (
            p0.0 + 2.0 / 3.0 * (control.0 - p0.0),
            p0.1 + 2.0 / 3.0 * (control.1 - p0.1),
        );
        let c2 = (
            end.0 + 2.0 / 3.0 * (control.0 - end.0),
            end.1 + 2.0 / 3.0 * (control.1 - end.1),
        );
        self.segs.push(Seg::Curve(c1.0, c1.1, c2.0, c2.1, end.0, end.1));
        self.cur = end;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let c1 = self.pt(x1, y1);
        let c2 = self.pt(x2, y2);
        let end = self.pt(x, y);
        self.segs.push(Seg::Curve(c1.0, c1.1, c2.0, c2.1, end.0, end.1));
        self.cur = end;
    }

    fn close(&mut self) {
        self.segs.push(Seg::Close);
        self.cur = self.start;
    }
}

/// 排好版的一行文字。
struct Layout {
    segs: Vec<Seg>,
    /// 真实 advance 之和（pt），用于水平居中。
    advance: f64,
    /// 字形墨迹包围盒（pt）：x_min, y_min, x_max, y_max。
    bbox: (f64, f64, f64, f64),
}

/// 逐字符取字形轮廓并排版；没有可用字体或一个字形都取不到时返回 None。
fn layout(text: &str) -> Option<Layout> {
    let font = font::outline_font()?;
    let face = font.face();
    let upem = face.units_per_em().max(1) as f64;
    let scale = FONT_SIZE / upem;

    let mut segs: Vec<Seg> = Vec::new();
    let mut pen_x = 0.0f64;
    let mut bbox: Option<(f64, f64, f64, f64)> = None;
    let mut drawn = 0usize;

    for ch in text.chars() {
        if ch.is_control() {
            continue;
        }
        let Some(gid) = face.glyph_index(ch) else {
            continue;
        };
        let advance = face
            .glyph_hor_advance(gid)
            .map(|a| a as f64)
            .unwrap_or(upem);

        let mut sink = OutlineSink {
            segs: &mut segs,
            pen_x,
            scale,
            cur: (0.0, 0.0),
            start: (0.0, 0.0),
        };
        if let Some(rect) = face.outline_glyph(gid, &mut sink) {
            let x0 = pen_x + rect.x_min as f64 * scale;
            let y0 = rect.y_min as f64 * scale;
            let x1 = pen_x + rect.x_max as f64 * scale;
            let y1 = rect.y_max as f64 * scale;
            bbox = Some(match bbox {
                None => (x0, y0, x1, y1),
                Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
            });
            drawn += 1;
        }
        // 空白字形（空格等）只推进不动笔
        pen_x += advance * scale;
    }

    if drawn == 0 {
        return None;
    }
    Some(Layout { segs, advance: pen_x, bbox: bbox? })
}

/// 生成内容流：白底 + 一行矢量文字（失败时降级为浅灰横线）。
fn build_content(w: f64, h: f64, text: &str) -> Vec<u8> {
    let mut out = String::with_capacity(8 * 1024);

    // 先铺白底，避免透明背景叠加出灰
    // 注意：操作符和下一个 token 之间必须有空白，否则会粘成 "1 g0" 这种非法关键字
    out.push_str("1 g\n");
    let _ = write!(out, "0 0 {} {} re f\n", num(w), num(h));

    let Some(layout) = layout(text) else {
        // 降级：不返回 Err，保证 impose 还能跑完
        log::warn!(
            "分隔页文字无法矢量化（没有可用字体或一个字形都取不到），退化为浅灰横线：{text:?}"
        );
        let _ = write!(
            out,
            "0.75 G 0.5 w\n{} {} m {} {} l S\n",
            num(w * 0.2),
            num(h * 0.5),
            num(w * 0.8),
            num(h * 0.5)
        );
        return out.into_bytes();
    };

    let (_x0, y0, _x1, y1) = layout.bbox;
    let ink_h = (y1 - y0).max(0.0);
    // 水平：按真实 advance 之和居中；垂直：按墨迹包围盒居中（视觉居中）
    let dx = (w - layout.advance) / 2.0;
    let dy = (h - ink_h) / 2.0 - y0;

    let _ = write!(out, "{} g\n", num(TEXT_GRAY));
    for seg in layout.segs {
        match seg {
            Seg::Move(x, y) => {
                let _ = write!(out, "{} {} m\n", num(x + dx), num(y + dy));
            }
            Seg::Line(x, y) => {
                let _ = write!(out, "{} {} l\n", num(x + dx), num(y + dy));
            }
            Seg::Curve(x1, y1, x2, y2, x3, y3) => {
                let _ = write!(
                    out,
                    "{} {} {} {} {} {} c\n",
                    num(x1 + dx),
                    num(y1 + dy),
                    num(x2 + dx),
                    num(y2 + dy),
                    num(x3 + dx),
                    num(y3 + dy)
                );
            }
            Seg::Close => out.push_str("h\n"),
        }
    }
    // 所有字形共用一次非零环绕填充（TrueType 本身按 nonzero 定义，不能用 f*）
    out.push_str("f\n");
    out.into_bytes()
}

/// 数字格式化：3 位小数（0.001pt ≈ 0.00035mm，远小于任何打印精度），去掉尾零。
fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    let mut s = format!("{v:.3}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".to_string();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4: (f64, f64) = (595.28, 841.89);

    fn a4() -> Arc<Vec<u8>> {
        separator_pdf(A4.0, A4.1, "这是分隔线").expect("生成分隔页失败")
    }

    fn first_page(doc: &Document) -> lopdf::ObjectId {
        *doc.get_pages().values().next().expect("没有页面")
    }

    fn content_of(bytes: &[u8]) -> String {
        let doc = Document::load_mem(bytes).expect("重新加载失败");
        let id = first_page(&doc);
        String::from_utf8(doc.get_page_content(id)).expect("内容流应该是文本")
    }

    #[test]
    fn a4_single_page_and_media_box() {
        let bytes = a4();
        let doc = Document::load_mem(&bytes).expect("重新加载失败");
        let pages = doc.get_pages();
        assert_eq!(pages.len(), 1, "分隔页应该只有 1 页");

        let page = doc
            .get_object(first_page(&doc))
            .and_then(Object::as_dict)
            .expect("页面字典");
        let media_box = page
            .get(b"MediaBox")
            .and_then(Object::as_array)
            .expect("MediaBox");
        let vals: Vec<f32> = media_box
            .iter()
            .map(|o| o.as_float().expect("MediaBox 应该是数字"))
            .collect();
        assert_eq!(vals.len(), 4, "MediaBox 应有 4 个值");
        assert_eq!(vals[0], 0.0);
        assert_eq!(vals[1], 0.0);
        assert!((vals[2] - A4.0 as f32).abs() < 0.01, "宽 = {}", vals[2]);
        assert!((vals[3] - A4.1 as f32).abs() < 0.01, "高 = {}", vals[3]);
        eprintln!("A4 分隔页：1 页，MediaBox = {vals:?}");
    }

    #[test]
    fn pdf_is_tiny() {
        let bytes = a4();
        eprintln!("矢量分隔页体积：{} 字节", bytes.len());
        assert!(
            bytes.len() < 40 * 1024,
            "矢量分隔页应 < 40KB，实际 {} 字节",
            bytes.len()
        );
    }

    #[test]
    fn no_font_objects_are_embedded() {
        let bytes = a4();
        let doc = Document::load_mem(&bytes).expect("重新加载失败");
        for (id, obj) in &doc.objects {
            let dict = match obj {
                Object::Dictionary(d) => Some(d),
                Object::Stream(s) => Some(&s.dict),
                _ => None,
            };
            let Some(dict) = dict else { continue };
            if let Ok(subtype) = dict.get(b"Subtype").and_then(Object::as_name) {
                assert_ne!(
                    subtype,
                    b"Font".as_slice(),
                    "对象 {id:?} 不应是字体对象"
                );
            }
            for key in [b"FontFile".as_slice(), b"FontFile2", b"FontFile3"] {
                assert!(
                    !dict.has(key),
                    "对象 {id:?} 不应含 {}（字体文件）",
                    String::from_utf8_lossy(key)
                );
            }
        }
        eprintln!("已确认：{} 个对象里没有任何字体对象", doc.objects.len());
    }

    #[test]
    fn content_is_vector_paths_not_image() {
        let content = content_of(&a4());
        let tokens: Vec<&str> = content.split_ascii_whitespace().collect();
        for op in ["m", "l", "c", "h", "f"] {
            assert!(
                tokens.iter().any(|t| *t == op),
                "内容流缺少路径算子 {op}，前 300 字节：{}",
                &content[..content.len().min(300)]
            );
        }
        // 不能是图像引用（旧实现是 q ... cm /Im0 Do Q）
        assert!(!content.contains("/Im0"), "不应引用图像 XObject");
        assert!(!tokens.iter().any(|t| *t == "Do"), "不应出现 Do 算子");
        assert!(!tokens.iter().any(|t| *t == "Bi"), "不应出现内联图像");
        // 也不能退回到文字算子（那就说明又嵌字体了）
        for op in ["Tj", "TJ", "Tf", "BT"] {
            assert!(
                !tokens.iter().any(|t| *t == op),
                "内容流不应有文字算子 {op}"
            );
        }
        let move_ops = tokens.iter().filter(|t| **t == "m").count();
        let curve_ops = tokens.iter().filter(|t| **t == "c").count();
        eprintln!(
            "内容流 {} 字节：{move_ops} 个 m，{} 个 l，{curve_ops} 个 c，1 个 f",
            content.len(),
            tokens.iter().filter(|t| **t == "l").count()
        );
        assert!(move_ops > 0 && curve_ops > 0, "应当是矢量轮廓路径");
    }

    #[test]
    fn different_text_gives_different_content() {
        let a = content_of(&separator_pdf(A4.0, A4.1, "这是分隔线").unwrap());
        let b = content_of(&separator_pdf(A4.0, A4.1, "那是分隔页").unwrap());
        assert_ne!(a, b, "不同文字必须产生不同内容流（缓存 key 可能写错了）");
        eprintln!("不同文字内容流不同：{} vs {} 字节", a.len(), b.len());
    }

    /// 操作符之间必须用空白隔开：曾经写成 `"1 g" + "0 0 …"` 粘成 `1 g0 0 …`，
    /// 严格解析器会把它当成一个未知关键字 `g0`，白底/灰字就丢了。
    #[test]
    fn content_tokens_are_well_separated() {
        let content = content_of(&a4());
        const OPS: [&str; 10] = ["g", "re", "f", "m", "l", "c", "h", "S", "G", "w"];
        for tok in content.split_ascii_whitespace() {
            if tok.parse::<f64>().is_ok() {
                continue;
            }
            assert!(
                OPS.contains(&tok),
                "内容流里有无法识别的 token：{tok:?}（操作符和数字之间漏了空白？）"
            );
        }
        assert!(
            content.starts_with("1 g\n0 0 "),
            "白底应从 \"1 g\\n\" 开始，实际开头：{:?}",
            &content[..content.len().min(40)]
        );
    }

    #[test]
    fn cache_returns_same_arc() {
        let a = separator_pdf(A4.0, A4.1, "缓存测试").unwrap();
        let b = separator_pdf(A4.0, A4.1, "缓存测试").unwrap();
        assert!(Arc::ptr_eq(&a, &b), "同尺寸同文字应命中缓存");
    }

    #[test]
    fn empty_text_degrades_gracefully() {
        // 一个字形都没有时不能 Err（否则整次拼版挂掉），退化成浅灰横线
        let bytes = separator_pdf(A4.0, A4.1, "").unwrap();
        let doc = Document::load_mem(&bytes).unwrap();
        assert_eq!(doc.get_pages().len(), 1);
        let content = content_of(&bytes);
        let tokens: Vec<&str> = content.split_ascii_whitespace().collect();
        for op in ["m", "l", "S"] {
            assert!(tokens.iter().any(|t| *t == op), "降级内容缺少算子 {op}");
        }
        eprintln!("空文字降级为横线：{} 字节", content.len());
    }
}
