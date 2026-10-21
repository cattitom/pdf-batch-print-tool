//! 页码：在内容页右下角盖「1,2,3…」，每个文件重新从 1 开始。
//!
//! 旧实现是「拼好 → 序列化 → pdfium 整份重新打开 → 逐页 watermark → 再保存」，
//! 实测近乎平方增长（1000 页 11.5s，5000 页 505s）。而页码只是一行 ASCII 数字，
//! 完全可以在 lopdf 的文档对象上直接追加一个几十字节的内容流：
//! O(n)、常数极小，且用 Helvetica 标准字体（不嵌入，体积零膨胀）。

use crate::error::ImposeError;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

/// 资源名，加前缀避免与源文件里的 `/F1` 冲突。
const FONT_NAME: &[u8] = b"PNum";
const FONT_SIZE: f64 = 8.0;
/// 右下角留白（pt）：右边距 / 基线高度。
const MARGIN_RIGHT: f64 = 6.0;
const BASELINE: f64 = 4.0;
/// Helvetica 的填充灰（#787878，与旧版 PdfColor(120,120,120) 一致）。
const GRAY: f64 = 0.47;

/// 一页的落款信息：页对象、最终页面宽高（pt）、要盖的页码（`None` = 跳过，如分隔页）。
pub struct NumberedPage {
    pub id: ObjectId,
    pub width: f64,
    pub height: f64,
    pub label: Option<String>,
}

/// 给所有带 `label` 的页追加页码内容流。
pub fn add_page_numbers(
    doc: &mut Document,
    pages: &[NumberedPage],
) -> Result<(), ImposeError> {
    if pages.iter().all(|p| p.label.is_none()) {
        return Ok(());
    }

    // 标准字体 14 之一：不嵌入字形，所以输出体积不涨
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });

    for page in pages {
        let Some(text) = &page.label else { continue };
        let stream_id = doc.add_object(Stream::new(Dictionary::new(), text_stream(page.width, text)));
        add_font_resource(doc, page.id, font_id)?;
        append_content(doc, page.id, stream_id)?;
    }
    Ok(())
}

/// 一行文字的内容流（`q … Q` 包裹，避免影响原页面图形状态）。
fn text_stream(width: f64, text: &str) -> Vec<u8> {
    // Helvetica 数字字宽 556/1000 em，8pt → 4.448pt/字符
    let text_width = text.chars().count() as f64 * FONT_SIZE * 0.556;
    let x = (width - text_width - MARGIN_RIGHT).max(0.0);
    let name = std::str::from_utf8(FONT_NAME).unwrap_or("PNum");
    format!(
        "q {GRAY} g BT /{name} {FONT_SIZE} Tf 1 0 0 1 {x:.2} {BASELINE} Tm ({}) Tj ET Q\n",
        escape(text)
    )
    .into_bytes()
}

/// 括号/反斜杠转义（页码是数字，这里只是防御）。
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '(' | ')' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 在页面 Resources/Font 里登记页码字体（按页复制一份资源字典，不动共享对象）。
fn add_font_resource(
    doc: &mut Document,
    page_id: ObjectId,
    font_id: ObjectId,
) -> Result<(), ImposeError> {
    let page = doc.get_object(page_id)?.clone();
    let Object::Dictionary(page_dict) = page else {
        return Err(ImposeError::pdf("页面不是字典对象"));
    };

    // Resources 可能是直接字典，也可能是指向共享对象的引用
    let mut resources = match page_dict.get(b"Resources") {
        Ok(Object::Reference(id)) => doc.get_object(*id)?.clone(),
        Ok(obj) => obj.clone(),
        Err(_) => Object::Dictionary(Dictionary::new()),
    };
    let Object::Dictionary(res_dict) = &mut resources else {
        // 资源不是字典（极少见）：直接替换成只含页码字体的资源
        let fresh = dictionary! { "Font" => dictionary! { String::from_utf8_lossy(FONT_NAME).to_string() => Object::Reference(font_id) } };
        if let Object::Dictionary(pd) = doc.get_object_mut(page_id)? {
            pd.set("Resources", fresh);
        }
        return Ok(());
    };

    // 字体字典同样可能是引用
    let mut fonts = match res_dict.get(b"Font") {
        Ok(Object::Reference(id)) => doc.get_object(*id)?.clone(),
        Ok(obj) => obj.clone(),
        Err(_) => Object::Dictionary(Dictionary::new()),
    };
    match &mut fonts {
        Object::Dictionary(fonts_dict) => {
            fonts_dict.set(FONT_NAME, Object::Reference(font_id));
            res_dict.set("Font", fonts);
        }
        _ => {
            res_dict.set(
                "Font",
                dictionary! { String::from_utf8_lossy(FONT_NAME).to_string() => Object::Reference(font_id) },
            );
        }
    }

    if let Object::Dictionary(pd) = doc.get_object_mut(page_id)? {
        pd.set("Resources", resources);
    }
    Ok(())
}

/// 把页码流追加到页面内容流末尾（保留原有单引用/数组两种形态）。
fn append_content(
    doc: &mut Document,
    page_id: ObjectId,
    stream_id: ObjectId,
) -> Result<(), ImposeError> {
    let Object::Dictionary(pd) = doc.get_object_mut(page_id)? else {
        return Err(ImposeError::pdf("页面不是字典对象"));
    };
    let mut parts = match pd.get(b"Contents") {
        Ok(Object::Array(items)) => items.clone(),
        Ok(Object::Reference(id)) => vec![Object::Reference(*id)],
        _ => Vec::new(),
    };
    parts.push(Object::Reference(stream_id));
    pd.set("Contents", Object::Array(parts));
    Ok(())
}
