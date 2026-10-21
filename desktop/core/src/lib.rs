//! PDF 批量打印工具核心库。
//!
//! - [`impose`]：拼版合并（凑页复制 / 旋转 / 裁剪 / 分隔页）
//! - [`sep`]：分隔页生成（按尺寸+文字缓存，避免重复付字体嵌入代价）
//! - [`numbers`]：页码（lopdf 直接追加内容流，O(n)）
//! - [`crop`]：内容包围盒检测（自动去白边）
//! - [`labels`]：入库箱唛批量生成（Code128 + 二维码 + 中文排版）
//! - [`flatten`]：整份扁平化成 300dpi 灰度图，压掉 CJK 字体嵌入的体积
//! - [`db`]：SQLite 数据层（生成历史 / 箱唛单据 / 用户偏好）
//!
//! 该库与 UI 无关，供 Tauri 客户端调用，可独立 `cargo test`。

pub mod crop;
pub mod db;
pub mod error;
pub mod flatten;
pub mod font;
pub mod impose;
pub mod labels;
pub mod numbers;
pub mod sep;

#[cfg(test)]
pub mod testutil;

pub use error::{ImposeError, ImposeResult};

/// 供桥接层映射数据库错误（Db 的签名本来就暴露 rusqlite::Result）。
pub use rusqlite;

pub use crop::{detect_content_bbox, render_page_bgra, render_pages, CropSpec};
pub use db::{Db, LabelDocEntry, LabelDocInput, PrintHistoryEntry, PrintHistoryInput};
pub use flatten::flatten_pdf;
pub use impose::{
    impose, impose_to, pdf_info, FileInput, ImposeOptions, ImposeStats, MM_TO_PT, PdfInfo,
};
pub use labels::{
    build_plan, generate_box_labels, generate_label_preview, render_preview, LabelDocSpec, LabelSku,
    ParsedQuantity,
};
