//! 内容包围盒检测（「自动去白边」）与裁剪参数。
//!
//! 原理与 Streamlit 版一致：把页面光栅化成位图，找非白像素的包围盒，
//! 再换算回 PDF 点坐标（Y 轴翻转：渲染原点在左上，PDF 原点在左下）。

use crate::error::ImposeError;
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// 共享的 PDFium 实例（库只需绑定一次）。
pub fn pdfium() -> &'static Pdfium {
    static PDFIUM: OnceLock<Pdfium> = OnceLock::new();
    PDFIUM.get_or_init(|| {
        pdfium_bundled::bind_pdfium_silent().expect("绑定 PDFium 失败（pdfium-bundled）")
    })
}

/// 裁剪方式：与 Streamlit 版的「自动去白边 / 手动裁剪」对应。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CropSpec {
    /// 自动去白边：检测内容包围盒后四边各加 margin_mm 毫米留白
    Auto { margin_mm: f64 },
    /// 已计算好的绝对裁剪矩形（页面坐标，pt）
    Rect { x0: f64, y0: f64, x1: f64, y1: f64 },
    /// 手动裁剪：按毫米裁掉 上/下/左/右
    Manual {
        top_mm: f64,
        bottom_mm: f64,
        left_mm: f64,
        right_mm: f64,
    },
}

pub const MM_TO_PT: f64 = 72.0 / 25.4;

/// 检测第一页的内容包围盒，返回 PDF 点坐标 (x0, y0, x1, y1)。
///
/// `dpi` 越高越精确、越慢；与 Streamlit 版相同精度时建议 ≥72。
/// 全空白页返回 `None`。
pub fn detect_content_bbox(
    pdf_bytes: &[u8],
    dpi: f32,
) -> Result<Option<(f64, f64, f64, f64)>, ImposeError> {
    let pdfium = pdfium();
    let doc = pdfium.load_pdf_from_byte_slice(pdf_bytes, None)?;
    let page = doc.pages().get(0)?;
    let (w, h) = (page.width().value as f64, page.height().value as f64);

    let cfg = PdfRenderConfig::new()
        .set_target_width((w / 72.0 * dpi as f64) as i32)
        .set_target_height((h / 72.0 * dpi as f64) as i32);
    let img = page.render_with_config(&cfg)?.as_image()?.to_rgb8();

    let mut x0 = img.width() as f64;
    let mut y0 = img.height() as f64;
    let mut x1 = 0.0f64;
    let mut y1 = 0.0f64;
    for py in 0..img.height() {
        for px in 0..img.width() {
            let p = img.get_pixel(px, py);
            if p[0] < 250 || p[1] < 250 || p[2] < 250 {
                let fx = px as f64;
                let fy = py as f64;
                x0 = x0.min(fx);
                y0 = y0.min(fy);
                x1 = x1.max(fx + 1.0);
                y1 = y1.max(fy + 1.0);
            }
        }
    }
    if x1 <= x0 {
        return Ok(None); // 全空白页
    }
    let k = 72.0 / dpi as f64;
    Ok(Some((
        x0 * k,
        h - y1 * k, // 翻转 Y：PDF 坐标系原点在左下
        x1 * k,
        h - y0 * k,
    )))
}

/// 把 [CropSpec] 解析成页面上的绝对裁剪矩形（pt）。
///
/// `page_size` 是应用裁剪前该页的 (宽, 高)，单位 pt。
pub fn crop_rect(
    spec: &CropSpec,
    page_size: (f64, f64),
) -> Result<(f64, f64, f64, f64), ImposeError> {
    let (w, h) = page_size;
    match spec {
        CropSpec::Auto { .. } => Err(ImposeError::invalid("Auto 裁剪需先经 auto_crop_rect 解析后再传入")),
        CropSpec::Rect { x0, y0, x1, y1 } => {
            if x1 <= x0 || y1 <= y0 {
                return Err(ImposeError::invalid("裁剪矩形无效"));
            }
            Ok((*x0, *y0, *x1, *y1))
        }
        CropSpec::Manual {
            top_mm,
            bottom_mm,
            left_mm,
            right_mm,
        } => {
            let x0 = left_mm * MM_TO_PT;
            let y0 = bottom_mm * MM_TO_PT;
            let x1 = w - right_mm * MM_TO_PT;
            let y1 = h - top_mm * MM_TO_PT;
            if x1 <= x0 || y1 <= y0 {
                return Err(ImposeError::invalid("手动裁剪裁掉的范围超过页面本身"));
            }
            Ok((x0, y0, x1, y1))
        }
    }
}

/// 自动去白边：检测包围盒并四边各加 margin_mm 留白，返回裁剪矩形（pt）。
pub fn auto_crop_rect(
    pdf_bytes: &[u8],
    margin_mm: f64,
    dpi: f32,
) -> Result<Option<(f64, f64, f64, f64)>, ImposeError> {
    let m = margin_mm * MM_TO_PT;
    Ok(detect_content_bbox(pdf_bytes, dpi)?
        .map(|(x0, y0, x1, y1)| (x0 - m, y0 - m, x1 + m, y1 + m)))
}

/// 渲染前 `max_pages` 页为 PNG 字节数组（UI 预览用，按页顺序）。
pub fn render_pages(
    pdf_bytes: &[u8],
    dpi: f32,
    max_pages: u32,
) -> Result<Vec<Vec<u8>>, ImposeError> {
    let doc = pdfium().load_pdf_from_byte_slice(pdf_bytes, None)?;
    let total = doc.pages().len() as u32;
    let mut out = Vec::new();
    for i in 0..total.min(max_pages.max(1)) {
        let page = doc.pages().get(i as i32)?;
        let (w, h) = (page.width().value as f64, page.height().value as f64);
        let cfg = PdfRenderConfig::new()
            .set_target_width((w / 72.0 * dpi as f64) as i32)
            .set_target_height((h / 72.0 * dpi as f64) as i32);
        let img = page.render_with_config(&cfg)?.as_image()?;
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png)?;
        out.push(buf.into_inner());
    }
    Ok(out)
}

/// 渲染单个页面为 **BGRA** 原始位图（供 Windows GDI 打印直接送印）。
///
/// 返回 `(宽, 高, BGRA 字节)`。GDI 的 32bpp DIB 期望 B,G,R 通道顺序，
/// 这里直接转好，调用方零拷贝即可用。
pub fn render_page_bgra(
    pdf_bytes: &[u8],
    page_index: u32,
    dpi: f32,
) -> Result<(u32, u32, Vec<u8>), ImposeError> {
    let doc = pdfium().load_pdf_from_byte_slice(pdf_bytes, None)?;
    let total = doc.pages().len();
    if page_index >= total as u32 {
        return Err(ImposeError::invalid(format!("页码越界：{page_index} / {total}")));
    }
    let page = doc.pages().get(page_index as i32)?;
    let (w, h) = (page.width().value as f64, page.height().value as f64);
    let cfg = PdfRenderConfig::new()
        .set_target_width((w / 72.0 * dpi as f64).round() as i32)
        .set_target_height((h / 72.0 * dpi as f64).round() as i32);
    let img = page.render_with_config(&cfg)?.as_image()?.to_rgba8();
    let (w, h) = img.dimensions();
    let mut buf = img.into_raw();
    // image 输出 RGBA，GDI 的 32bpp DIB 期望 BGRA：原地交换 R/B 通道
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Ok((w, h, buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture;

    #[test]
    fn bgra_render_dims_and_len() {
        let bytes = fixture("CROP-A4-3页.pdf");
        let (w, h, buf) = render_page_bgra(&bytes, 0, 150.0).unwrap();
        // A4 = 210×297mm ≈ 1240×1754 @150dpi（宽高互换取决于页面方向）
        assert_eq!(buf.len(), (w * h * 4) as usize, "BGRA 缓冲长度不符");
        assert!(
            (w as f64 - 210.0 / 25.4 * 150.0).abs() < 3.0
                || (w as f64 - 297.0 / 25.4 * 150.0).abs() < 3.0,
            "渲染宽度异常: {w}"
        );
        // 页码越界要报错
        assert!(render_page_bgra(&bytes, 99, 150.0).is_err());
    }

    #[test]
    fn bbox_matches_pymupdf_reference() {
        // 参考值来自 Streamlit 版 pymupdf 同算法输出（误差 ≤1pt）
        let bytes = fixture("CROP-A4-3页.pdf");
        let bbox = detect_content_bbox(&bytes, 72.0).unwrap().unwrap();
        assert!(
            (bbox.0 - 89.0).abs() <= 1.0
                && (bbox.1 - 380.89).abs() <= 1.0
                && (bbox.2 - 505.0).abs() <= 1.0
                && (bbox.3 - 460.89).abs() <= 1.0,
            "bbox 偏差过大: {:?}",
            bbox
        );
    }

    #[test]
    fn manual_crop_rect() {
        let r = crop_rect(
            &CropSpec::Manual {
                top_mm: 10.0,
                bottom_mm: 5.0,
                left_mm: 8.0,
                right_mm: 8.0,
            },
            (595.28, 841.89),
        )
        .unwrap();
        assert!((r.0 - 8.0 * MM_TO_PT).abs() < 1e-6);
        assert!((r.3 - (841.89 - 10.0 * MM_TO_PT)).abs() < 1e-6);
        // 裁过头要报错
        assert!(crop_rect(
            &CropSpec::Manual {
                top_mm: 500.0,
                bottom_mm: 500.0,
                left_mm: 0.0,
                right_mm: 0.0
            },
            (595.28, 841.89)
        )
        .is_err());
    }
}
