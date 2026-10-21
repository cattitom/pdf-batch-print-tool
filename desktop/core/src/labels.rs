//! 入库箱唛批量生成：一箱一页，Code128 条码（入库单号）+ 二维码（单号-S货架-箱号）
//! + 中文排版。标签规格固定 100×100mm（热敏标签纸只有这一种）。

use crate::error::ImposeError;
use crate::crop::pdfium;
use genpdf::elements::{Image as GImage, Paragraph};
use genpdf::fonts::{FontData, FontFamily};
use std::io::Cursor;

/// 标签尺寸，唯一规格：100×100mm（283.46 × 283.46 pt）。
pub const LABEL_MM: (u16, u16) = (100, 100);

/// 版式参数：(页边距 mm, 标题字号, 正文字号, 小字号, 条码高 px, 二维码边长 px)。
const LAYOUT: (f32, f32, f32, f32, u32, u32) = (6.0, 13.0, 11.0, 8.5, 40, 90);

/// 一个 SKU 的录入行。
///
/// 前后端都用 camelCase：`pcsPerBox` / `newFlag`。之前漏了 `rename_all`，
/// 前端传来的字段对不上、被 serde 默认值吞掉（每箱件数恒为 1）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct LabelSku {
    pub sku: String,
    #[serde(default = "default_boxes")]
    pub boxes: u32,
    #[serde(default = "default_pcs")]
    pub pcs_per_box: u32,
    #[serde(default)]
    pub quantities: String,
    #[serde(default = "default_true")]
    pub new_flag: bool,
}

fn default_boxes() -> u32 {
    1
}
fn default_pcs() -> u32 {
    1
}
fn default_true() -> bool {
    true
}

/// 箱唛任务整体参数。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LabelDocSpec {
    pub inbound_no: String,
    pub warehouse: String,
    pub client: String,
    pub shelf: String,
    pub skus: Vec<LabelSku>,
}

/// 展开后的「一箱」：箱唛生成的最小单元。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParsedQuantity {
    pub sku: String,
    /// 本箱件数
    pub qty: u32,
    /// 箱号（如 S1-001）
    pub box_no: String,
    /// 二维码编号（单号-S货架-箱号）
    pub qr_text: String,
    pub new_flag: bool,
}

/// 解析「每箱件数」文本，如 `250*6,167*2,166` → [250×6, 167×2, 166]。
/// 空文本返回 None（全部用每箱件数）。
pub fn parse_quantities(text: &str) -> Option<Vec<u32>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for part in text.split([',', '，']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((n, times)) = part.split_once(['*', '×', 'x']) {
            let n: u32 = n.trim().parse().ok()?;
            let times: u32 = times.trim().parse().ok()?;
            for _ in 0..times {
                out.push(n);
            }
        } else {
            out.push(part.parse().ok()?);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 把 SKU 明细展开成一箱一条的计划表（条码编号 = 单号-S货架-序号）。
pub fn build_plan(spec: &LabelDocSpec) -> Vec<ParsedQuantity> {
    let mut plan = Vec::new();
    let shelf = spec.shelf.trim();
    let mut seq = 0usize;
    for sku in &spec.skus {
        let name = sku.sku.trim();
        if name.is_empty() {
            continue;
        }
        let quantities = parse_quantities(&sku.quantities)
            .unwrap_or_else(|| vec![sku.pcs_per_box; sku.boxes.max(1) as usize]);
        for (_i, qty) in quantities.iter().enumerate() {
            seq += 1;
            plan.push(ParsedQuantity {
                sku: name.to_string(),
                qty: *qty,
                box_no: format!("S{}-{:03}", shelf, seq),
                qr_text: format!("{}-S{}-{:03}", spec.inbound_no.trim(), shelf, seq),
                new_flag: sku.new_flag,
            });
        }
    }
    plan
}

// ---------- 图片素材 ----------

fn rgb_png_bytes(img: image::DynamicImage) -> Result<Vec<u8>, ImposeError> {
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img.to_rgb8()).write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

/// Code128 条码 PNG（RGB，无 alpha —— printpdf 不支持 alpha）。
fn code128_png(text: &str, height: u32) -> Result<Vec<u8>, ImposeError> {
    use barcoders::generators::image::Image;
    use barcoders::sym::code128::Code128;
    // 2.0 需要显式 START-B
    let code = Code128::new(format!("Ɓ{}", text))
        .map_err(|e| ImposeError::invalid(format!("条码内容不合法：{e}")))?;
    let png = Image::png(height)
        .generate(&code.encode())
        .map_err(|e| ImposeError::render(format!("条码渲染失败：{e}")))?;
    rgb_png_bytes(image::load_from_memory(&png)?)
}

/// 二维码 PNG（RGB）。
fn qr_png(text: &str, size: u32) -> Result<Vec<u8>, ImposeError> {
    use image::Luma;
    use qrcode::QrCode;
    let code = QrCode::new(text.as_bytes())
        .map_err(|e| ImposeError::invalid(format!("二维码内容不合法：{e}")))?;
    rgb_png_bytes(image::DynamicImage::ImageLuma8(
        code.render::<Luma<u8>>()
            .min_dimensions(size, size)
            .max_dimensions(size, size)
            .build(),
    ))
}

// ---------- 字体 ----------

fn load_cjk_font() -> Result<FontFamily<FontData>, ImposeError> {
    // 候选表由 font 模块统一维护；genpdf/rustybuzz 解析不了 .ttc 字形集合，
    // 直接跳过（否则要白读 20MB 才回退到下一个候选）
    for path in crate::font::candidate_paths() {
        if path.ends_with(".ttc") {
            continue;
        }
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(fd) = FontData::new(bytes, None) {
                return Ok(FontFamily {
                    regular: fd.clone(),
                    bold: fd.clone(),
                    italic: fd.clone(),
                    bold_italic: fd,
                });
            }
        }
    }
    Err("未找到中文字体（需要 simhei.ttf / msyh.ttc 等）".into())
}

// ---------- 生成 ----------

/// 批量生成箱唛，一箱一页，返回 PDF 字节。
/// 生成箱唛 PDF（原始版：genpdf 排版 + 整份字体嵌入，体积大，仅内部使用）。
pub fn generate_box_labels_raw(spec: &LabelDocSpec) -> Result<Vec<u8>, ImposeError> {
    let plan = build_plan(spec);
    if plan.is_empty() {
        return Err("请至少填写一个 SKU".into());
    }

    let family = load_cjk_font()?;
    let mut doc = genpdf::Document::new(family);
    let (mm_w, mm_h) = LABEL_MM;
    doc.set_paper_size(genpdf::Size::new(mm_w, mm_h));
    let mut deco = genpdf::SimplePageDecorator::new();
    let (margin, title_fs, big_fs, small_fs, bar_h, qr_sz) = LAYOUT;
    deco.set_margins(genpdf::Margins::all(margin));
    doc.set_page_decorator(deco);

    let bold = genpdf::style::Style::new().bold().with_font_size(title_fs.round() as u8);
    let normal_big = genpdf::style::Style::new().with_font_size(big_fs.round() as u8);
    let normal_small = genpdf::style::Style::new().with_font_size(small_fs.round() as u8);

    for (idx, item) in plan.iter().enumerate() {
        if idx > 0 {
            doc.push(genpdf::elements::PageBreak::new());
        }

        doc.push(genpdf::elements::StyledElement::new(
            Paragraph::new(format!("入库单号：{}", spec.inbound_no)),
            bold.clone(),
        ));
        doc.push(genpdf::elements::Break::new(0.4));

        // 条码（入库单号）
        let bar = GImage::from_reader(Cursor::new(code128_png(
            &spec.inbound_no,
            bar_h,
        )?))?;
        doc.push(bar);
        doc.push(genpdf::elements::Break::new(0.4));

        doc.push(genpdf::elements::StyledElement::new(
            Paragraph::new(format!(
                "客户：{}   仓库：{}",
                spec.client, spec.warehouse
            )),
            normal_small.clone(),
        ));
        doc.push(genpdf::elements::StyledElement::new(
            Paragraph::new(format!(
                "SKU：{}   箱号：{}   本箱 {} 件{}",
                item.sku,
                item.box_no,
                item.qty,
                if item.new_flag { "   New" } else { "" }
            )),
            normal_big.clone(),
        ));

        // 二维码（单号-S货架-箱号）
        let qr = GImage::from_reader(Cursor::new(qr_png(&item.qr_text, qr_sz)?))?;
        doc.push(qr);
    }

    let mut out = Vec::new();
    doc.render(&mut out)?;
    Ok(out)
}

/// 生成箱唛 PDF（对外接口）：生成后扁平化为 300dpi 灰度图，
/// 避免整份 CJK 字体嵌入导致几十 MB 的体积膨胀（见 flatten 模块说明）。
pub fn generate_box_labels(spec: &LabelDocSpec) -> Result<Vec<u8>, ImposeError> {
    let raw = generate_box_labels_raw(spec)?;
    crate::flatten::flatten_pdf(&raw, 300.0)
}

/// 用 pdfium 渲染第一页（给 UI 出预览缩略图；PNG 字节）。
pub fn render_preview(pdf_bytes: &[u8], dpi: f32) -> Result<Vec<u8>, ImposeError> {
    let pdfium = pdfium();
    let doc = pdfium.load_pdf_from_byte_slice(pdf_bytes, None)?;
    let page = doc.pages().get(0)?;
    let (w, h) = (page.width().value as f64, page.height().value as f64);
    let cfg = pdfium_render::prelude::PdfRenderConfig::new()
        .set_target_width((w / 72.0 * dpi as f64) as i32)
        .set_target_height((h / 72.0 * dpi as f64) as i32);
    let img = page.render_with_config(&cfg)?.as_image()?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LabelDocSpec {
        LabelDocSpec {
            inbound_no: "IB058260930RY".into(),
            warehouse: "NYC (NYC1)".into(),
            client: "江苏翰林供应链(2706058)".into(),
            shelf: "1".into(),
            skus: vec![
                LabelSku {
                    sku: "110-ZXD".into(),
                    boxes: 3,
                    pcs_per_box: 25,
                    quantities: String::new(),
                    new_flag: true,
                },
                LabelSku {
                    sku: "220-ABC".into(),
                    boxes: 0,
                    pcs_per_box: 0,
                    quantities: "10*2,7".into(), // 明细优先 → 3 箱
                    new_flag: false,
                },
            ],
        }
    }

    #[test]
    fn quantities_parsing() {
        assert_eq!(parse_quantities(""), None);
        assert_eq!(
            parse_quantities("250*6,167*2,166"),
            Some(vec![250, 250, 250, 250, 250, 250, 167, 167, 166])
        );
        assert_eq!(parse_quantities("10*2,7"), Some(vec![10, 10, 7]));
    }

    #[test]
    fn plan_expansion() {
        let plan = build_plan(&spec());
        // SKU1: 3 箱 × 25 件；SKU2: 明细 3 箱
        assert_eq!(plan.len(), 6);
        assert_eq!(plan[0].box_no, "S1-001");
        assert_eq!(plan[0].qr_text, "IB058260930RY-S1-001");
        assert_eq!(plan[0].qty, 25);
        assert_eq!(plan[3].sku, "220-ABC");
        assert_eq!(plan[3].qty, 10);
        assert!(plan[0].new_flag && !plan[5].new_flag);
    }

    #[test]
    fn generates_100x100() {
        let pdf = generate_box_labels(&spec()).unwrap();
        let doc = lopdf::Document::load_mem(&pdf).unwrap();
        assert_eq!(doc.get_pages().len(), 6);

        // 页面必须是 100×100mm（283.46pt×283.46pt），容差 1pt
        let pdfium = pdfium();
        let rendered = pdfium.load_pdf_from_byte_slice(&pdf, None).unwrap();
        let page = rendered.pages().get(0).unwrap();
        let (w, h) = (page.width().value, page.height().value);
        assert!(
            (w - 283.46).abs() < 1.0 && (h - 283.46).abs() < 1.0,
            "页面尺寸应为 100×100mm，实际 {w}×{h}pt"
        );

        // 渲染第一页确认能画出来
        let png = render_preview(&pdf, 72.0).unwrap();
        assert!(png.len() > 1000);
    }
}
