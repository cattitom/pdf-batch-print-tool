//! PDF 扁平化：把每页渲染成灰度位图，重建成「每页一张图」的 PDF。
//!
//! 用途：pdfium / genpdf 嵌入 CJK 字体时都会把**整份字体文件**塞进 PDF
//! （如 msyh 19.7MB、simhei 10MB），导致几页的文档膨胀到几十 MB。
//! 网页版 reportlab 会做字形子集化所以很小；Rust 生态这两个库都不子集化，
//! 因此生成后统一扁平化：文件只剩图像，体积回到几十 KB，300dpi 打印质量足够
//! （热敏标签常见 203dpi，激光 600dpi），条码/二维码扫描无影响。

use crate::error::ImposeError;
use crate::crop::pdfium;
use lopdf::{dictionary, Dictionary, Document, Object, Stream};
use pdfium_render::prelude::*;

/// 把 PDF 每页渲染成灰度图并重建成新 PDF（打印用扁平化）。
///
/// `dpi` 建议 300：A4 单页约 3508×2480 灰度，压缩后通常 100–300KB。
pub fn flatten_pdf(pdf_bytes: &[u8], dpi: f32) -> Result<Vec<u8>, ImposeError> {
    let src = pdfium().load_pdf_from_byte_slice(pdf_bytes, None)?;
    let total = src.pages().len();

    let mut out = Document::with_version("1.7");
    let pages_id = out.new_object_id();
    let mut kids: Vec<Object> = Vec::with_capacity(total as usize);

    for i in 0..total {
        let page = src.pages().get(i as i32)?;
        let (pw, ph) = (page.width().value, page.height().value);
        let cfg = PdfRenderConfig::new()
            .set_target_width(((pw as f64 / 72.0 * dpi as f64).round() as i32).max(1))
            .set_target_height(((ph as f64 / 72.0 * dpi as f64).round() as i32).max(1));
        let img = page.render_with_config(&cfg)?.as_image()?.to_luma8();
        let (iw, ih) = img.dimensions();

        // 图像 XObject（灰度 8bit，Flate 压缩）
        let mut idict = Dictionary::new();
        idict.set("Type", "XObject");
        idict.set("Subtype", "Image");
        idict.set("Width", Object::Integer(iw as i64));
        idict.set("Height", Object::Integer(ih as i64));
        idict.set("ColorSpace", "DeviceGray");
        idict.set("BitsPerComponent", Object::Integer(8));
        let mut stream = Stream::new(idict, img.into_raw());
        stream.compress()?;
        let img_id = out.add_object(stream);

        // 内容流：把图像铺满整页
        let content = Stream::new(
            Dictionary::new(),
            format!("q {} 0 0 {} 0 0 cm /Im0 Do Q", pw, ph).into_bytes(),
        );
        let content_id = out.add_object(content);

        let page_dict = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Real(pw),
                Object::Real(ph),
            ],
            "Resources" => dictionary! {
                "XObject" => dictionary! { "Im0" => img_id },
            },
            "Contents" => content_id,
        };
        kids.push(Object::Reference(out.add_object(page_dict)));
    }

    let pages = dictionary! {
        "Type" => "Pages",
        "Kids" => kids,
        "Count" => Object::Integer(total as i64),
    };
    out.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id = out.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    out.trailer.set("Root", catalog_id);

    let mut buf = Vec::new();
    out.save_to(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_keeps_page_count_and_shrinks() {
        // 用箱唛生成器造一份带 CJK 字体的 PDF（原始约 40MB）
        let spec = crate::labels::LabelDocSpec {
            inbound_no: "FLATTEN".into(),
            warehouse: "W".into(),
            client: "C".into(),
            shelf: "1".into(),
            skus: vec![crate::labels::LabelSku {
                sku: "A".into(),
                boxes: 2,
                pcs_per_box: 10,
                quantities: String::new(),
                new_flag: true,
            }],
        };
        let raw = crate::labels::generate_box_labels_raw(&spec).unwrap();
        assert!(raw.len() > 1_000_000, "未扁平化时应是有字体嵌入的大文件: {}", raw.len());

        let small = flatten_pdf(&raw, 300.0).unwrap();
        let doc = Document::load_mem(&small).unwrap();
        assert_eq!(doc.get_pages().len(), 2);
        assert!(
            small.len() < 400_000,
            "扁平化后应 < 400KB，实际 {}",
            small.len()
        );
        eprintln!("扁平化：{} 字节 → {} 字节", raw.len(), small.len());
    }
}
