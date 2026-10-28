//! 产品标签（50 × 30 mm）：Code128 + 编码 + 中文名，一页一张。
//!
//! 与 [`crate::labels`]（100×100 入库箱唛、genpdf 流式排版）不同，这里用 printpdf
//! 按绝对坐标排版，位置对齐客户给的 `document1.pdf`（误差 < 0.5pt）。
//!
//! 字体整份嵌入后统一扁平化为 300dpi 灰度图：printpdf/genpdf 都不做字形子集化，
//! simhei 10MB 直接塞进 PDF 会让三页文件变成十几 MB（见 flatten 模块说明）。

use crate::error::ImposeError;
use barcoders::sym::code128::Code128;
use printpdf::{
    Color, Greyscale, IndirectFontRef, Line, Mm, PdfDocument, PdfLayerReference, Point,
};
use std::io::{BufWriter, Cursor};

/// 标签页面尺寸（mm）。
pub const LABEL_W_MM: f64 = 50.0;
pub const LABEL_H_MM: f64 = 30.0;

const PT_PER_MM: f64 = 72.0 / 25.4;

// ---- 版式（mm，左下角为原点，数值由 document1.pdf 反推） ----
/// 条码左边缘、距页顶、宽度、高度
const BAR_X_MM: f64 = 1.418;
const BAR_TOP_MM: f64 = 1.756;
const BAR_W_MM: f64 = 45.24;
const BAR_H_MM: f64 = 13.64;
/// 文字左边缘
const LEFT_MM: f64 = 2.476;
/// 编码 / 名称 / 条码数字的基线（距页底）
const BAR_TEXT_BASELINE_MM: f64 = 12.73;
const CODE_BASELINE_MM: f64 = 7.10;
const NAME_BASELINE_MM: f64 = 2.865;
/// 字号（pt）
const BAR_TEXT_FONT_PT: f64 = 5.0;
const CODE_FONT_PT: f64 = 10.0;
const NAME_FONT_PT: f64 = 10.0;

/// 产品标签的一行：编码 + 名称 + 份数。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct ProductLabelItem {
    /// 条码与编码行内容，如 `CX-RMG-04`
    pub code: String,
    /// 中文名，如 `染眉膏-04`（可空）
    #[serde(default)]
    pub name: String,
    /// 这个标签打印几张（>=1）
    #[serde(default = "default_copies")]
    pub copies: u32,
}

fn default_copies() -> u32 {
    1
}

/// 过滤空行后实际会生成的标签张数。
pub fn total_labels(items: &[ProductLabelItem]) -> usize {
    items
        .iter()
        .filter(|i| !i.code.trim().is_empty())
        .map(|i| i.copies.max(1) as usize)
        .sum()
}

/// 批量生成产品标签 PDF（每张一页，50×30mm），已扁平化，体积几十 KB。
pub fn generate_product_labels(items: &[ProductLabelItem]) -> Result<Vec<u8>, ImposeError> {
    let raw = render(items, false)?;
    crate::flatten::flatten_pdf(&raw, 300.0)
}

/// 只排第一行、只出一页，供 UI 实时预览（不做扁平化，保持矢量清晰）。
pub fn generate_product_label_preview(items: &[ProductLabelItem]) -> Result<Vec<u8>, ImposeError> {
    render(items, true)
}

/// 排版：`preview = true` 时只出第一页。
fn render(items: &[ProductLabelItem], preview: bool) -> Result<Vec<u8>, ImposeError> {
    let mut rows: Vec<&ProductLabelItem> = items
        .iter()
        .filter(|i| !i.code.trim().is_empty())
        .collect();
    if rows.is_empty() {
        return Err(ImposeError::invalid("请至少填写一行编码"));
    }
    // 预览只排第一行（其余行不参与，避免多出空白页）
    if preview {
        rows.truncate(1);
    }

    let cjk_bytes = load_cjk_font()?;
    let latin_bytes = load_latin_font().unwrap_or_else(|| cjk_bytes.clone());

    let (doc, page0, layer0) =
        PdfDocument::new("产品标签", Mm(LABEL_W_MM), Mm(LABEL_H_MM), "Layer 1");
    let cjk = doc
        .add_external_font(Cursor::new(cjk_bytes))
        .map_err(|e| ImposeError::render(format!("中文字体加载失败：{e}")))?;
    let latin = doc
        .add_external_font(Cursor::new(latin_bytes))
        .map_err(|e| ImposeError::render(format!("西文字体加载失败：{e}")))?;

    let mut page = page0;
    let mut layer_idx = layer0;
    let mut first = true;

    for item in &rows {
        let copies = if preview { 1 } else { item.copies.max(1) };
        for _ in 0..copies {
            if !first {
                let (p, l) = doc.add_page(Mm(LABEL_W_MM), Mm(LABEL_H_MM), "Layer 1");
                page = p;
                layer_idx = l;
            }
            first = false;

            let layer = doc.get_page(page).get_layer(layer_idx);
            draw_label(&layer, item, &cjk, &latin)?;
        }
    }

    let mut out = Vec::new();
    {
        let mut writer = BufWriter::new(Cursor::new(&mut out));
        doc.save(&mut writer)
            .map_err(|e| ImposeError::render(format!("PDF 写入失败：{e}")))?;
    }
    Ok(out)
}

/// 画一张标签：条码 + 条码数字 + 编码行 + 名称行。
fn draw_label(
    layer: &PdfLayerReference,
    item: &ProductLabelItem,
    cjk: &IndirectFontRef,
    latin: &IndirectFontRef,
) -> Result<(), ImposeError> {
    layer.set_fill_color(Color::Greyscale(Greyscale::new(0.0, None)));
    layer.set_outline_thickness(0.0);

    // 条码：把 0/1 模块串里的连续 1 合并成一根黑条
    let bits = code128_bits(&item.code)?;
    let module = BAR_W_MM / bits.len() as f64;
    let y0 = LABEL_H_MM - BAR_TOP_MM - BAR_H_MM;
    let y1 = y0 + BAR_H_MM;

    let mut i = 0usize;
    while i < bits.len() {
        if bits[i] == 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < bits.len() && bits[i] == 1 {
            i += 1;
        }
        let x0 = BAR_X_MM + start as f64 * module;
        let x1 = BAR_X_MM + i as f64 * module;
        layer.add_shape(Line {
            points: vec![
                (Point::new(Mm(x0), Mm(y0)), false),
                (Point::new(Mm(x1), Mm(y0)), false),
                (Point::new(Mm(x1), Mm(y1)), false),
                (Point::new(Mm(x0), Mm(y1)), false),
            ],
            is_closed: true,
            has_fill: true,
            has_stroke: false,
            is_clipping_path: false,
        });
    }

    // 条码下方的小字：按等宽近似居中
    layer.use_text(
        item.code.trim(),
        BAR_TEXT_FONT_PT,
        Mm(bar_text_x(&item.code)),
        Mm(BAR_TEXT_BASELINE_MM),
        latin,
    );

    // 编码行 / 名称行
    layer.use_text(
        item.code.trim(),
        CODE_FONT_PT,
        Mm(LEFT_MM),
        Mm(CODE_BASELINE_MM),
        latin,
    );
    if !item.name.trim().is_empty() {
        layer.use_text(
            item.name.trim(),
            NAME_FONT_PT,
            Mm(LEFT_MM),
            Mm(NAME_BASELINE_MM),
            cjk,
        );
    }

    Ok(())
}

/// Code128B 模块串（bit=1 表示黑条，无静区）。
fn code128_bits(code: &str) -> Result<Vec<u8>, ImposeError> {
    let encoded = Code128::new(format!("Ɓ{}", code.trim()))
        .map_err(|e| ImposeError::invalid(format!("条码内容不合法（{code}）：{e:?}")))?;
    Ok(encoded.encode())
}

/// 条码下方小字的左边缘：按 0.58em 的等宽估算宽度后居中。
fn bar_text_x(code: &str) -> f64 {
    let text_w_mm = code.trim().chars().count() as f64 * 0.58 * BAR_TEXT_FONT_PT / PT_PER_MM;
    let center = BAR_X_MM + BAR_W_MM / 2.0;
    (center - text_w_mm / 2.0).max(0.5)
}

/// 中文字体：只接受 `.ttf`（printpdf/rusttype 解析不了 `.ttc` 字体集合）。
fn load_cjk_font() -> Result<Vec<u8>, ImposeError> {
    for path in crate::font::candidate_paths() {
        if path.to_ascii_lowercase().ends_with(".ttc") {
            continue;
        }
        if let Ok(bytes) = std::fs::read(path) {
            return Ok(bytes);
        }
    }
    Err(ImposeError::render(
        "未找到中文字体（需要 C:\\Windows\\Fonts\\simhei.ttf）",
    ))
}

/// 西文字体：Tahoma 最接近原版，缺失时退回 Arial / Segoe UI / 中文字体。
fn load_latin_font() -> Option<Vec<u8>> {
    const CANDIDATES: [&str; 3] = [
        r"C:\Windows\Fonts\tahoma.ttf",
        r"C:\Windows\Fonts\arial.ttf",
        r"C:\Windows\Fonts\segoeui.ttf",
    ];
    CANDIDATES.iter().find_map(|p| std::fs::read(p).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<ProductLabelItem> {
        vec![
            ProductLabelItem {
                code: "CX-RMG-04".into(),
                name: "染眉膏-04".into(),
                copies: 1,
            },
            ProductLabelItem {
                code: "CX-RMG-02".into(),
                name: "染眉膏-02".into(),
                copies: 2,
            },
        ]
    }

    #[test]
    fn code128_module_count() {
        // 9 个字符：START + 9 数据 + 校验 = 11 个 11 模块符号，加 STOP 13 模块 = 134
        assert_eq!(code128_bits("CX-RMG-04").unwrap().len(), 134);
    }

    #[test]
    fn generates_one_page_per_copy() {
        assert_eq!(total_labels(&items()), 3);
        let pdf = generate_product_labels(&items()).unwrap();
        let doc = lopdf::Document::load_mem(&pdf).unwrap();
        assert_eq!(doc.get_pages().len(), 3);

        let preview = generate_product_label_preview(&items()).unwrap();
        let pdoc = lopdf::Document::load_mem(&preview).unwrap();
        assert_eq!(pdoc.get_pages().len(), 1);

        let png = crate::render_preview(&preview, 96.0).unwrap();
        assert!(png.len() > 1000, "预览应能渲染出 PNG");
    }

    #[test]
    fn empty_items_is_invalid() {
        let empty = vec![ProductLabelItem {
            code: "  ".into(),
            name: String::new(),
            copies: 1,
        }];
        assert_eq!(
            generate_product_labels(&empty).unwrap_err().code(),
            "invalid"
        );
    }
    /// 人工检查用：导出矢量预览 + 扁平化成品（`cargo test product_label -- --ignored --nocapture`）。
    #[test]
    #[ignore = "人工检查用"]
    fn dump_preview_png() {
        let dir = std::env::temp_dir();

        let preview = generate_product_label_preview(&items()).unwrap();
        std::fs::write(dir.join("product_label_preview.pdf"), &preview).unwrap();
        let png = crate::render_preview(&preview, 300.0).unwrap();
        std::fs::write(dir.join("product_label_preview.png"), &png).unwrap();

        let final_pdf = generate_product_labels(&items()).unwrap();
        std::fs::write(dir.join("product_label_final.pdf"), &final_pdf).unwrap();
        let final_png = crate::render_preview(&final_pdf, 300.0).unwrap();
        std::fs::write(dir.join("product_label_final.png"), &final_png).unwrap();

        eprintln!("OUT = {}", dir.display());
        eprintln!("final pdf {} bytes", final_pdf.len());
    }
}




