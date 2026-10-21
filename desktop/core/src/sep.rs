//! 分隔页：一张「这是分隔线」的扁平化图像页。
//!
//! 生成一份分隔页的代价很高——要把整份 CJK 字体（约 20MB）嵌进 PDF，
//! 再按 300dpi 渲染成图像页压回去，实测约 1.3s。一次拼版里分隔页的
//! 尺寸和文字通常只有一种，因此按 `(宽, 高, 文字)` 缓存生成结果：
//! N 个文件只付一次代价。

use crate::error::ImposeError;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use pdfium_render::prelude::*;

use crate::crop::pdfium;
use crate::font;

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

/// 实际生成：pdfium 画一行中文 → 保存（整份字体嵌入）→ 扁平化成图像页。
fn render(w: f64, h: f64, text: &str) -> Result<Vec<u8>, ImposeError> {
    let pdfium = pdfium();
    let mut doc = pdfium.create_new_pdf()?;
    let size = PdfPagePaperSize::from_points(PdfPoints::new(w as f32), PdfPoints::new(h as f32));
    let mut page = doc.pages_mut().create_page_at_end(size)?;

    // 中文优先；找不到中文字体退回 Helvetica + 英文
    // 注意：CJK 必须以 CID 方式嵌入（is_cid=true），否则字形映射错乱
    let (display, font, font_size) = match font::cjk_font_bytes() {
        Some(bytes) => {
            let font = doc.fonts_mut().load_true_type_from_bytes(bytes, true)?;
            (text.to_string(), font, 28.0f32)
        }
        None => (
            "This is a separator".to_string(),
            doc.fonts_mut().helvetica_bold(),
            24.0f32,
        ),
    };

    // 粗略估算文字宽度做水平居中（CJK 近似全角、ASCII 近似半角）
    let width_est: f32 = display
        .chars()
        .map(|c| if c.is_ascii() { font_size * 0.55 } else { font_size })
        .sum();
    let mut text_obj = PdfPageTextObject::new(&doc, display, font, PdfPoints::new(font_size))?;
    let _ = text_obj.set_fill_color(PdfColor::new(51, 51, 51, 255));
    // 新建文本对象位于原点，translate 即为绝对定位
    text_obj.translate(
        PdfPoints::new(((w as f32) - width_est) / 2.0),
        PdfPoints::new((h as f32 - font_size) / 2.0),
    )?;
    page.objects_mut().add_text_object(text_obj)?;

    // 扁平化：pdfium 会把整份 CJK 字体（十几 MB）嵌入，转成图像页后只有几十 KB
    let raw = doc.save_to_bytes()?;
    crate::flatten::flatten_pdf(&raw, 300.0)
}
