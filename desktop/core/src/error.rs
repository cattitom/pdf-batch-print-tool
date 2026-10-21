//! 统一错误类型：给前端可分支的错误码，而不是一句人话字符串。
//!
//! 之前 core 里是 `Box<dyn Error>`、桥接层是 `Result<_, String>`，前端只能把整句
//! 中文 `alert` 出来，无法区分「文件被占用」「PDF 损坏」「磁盘满」。
//! 现在每个错误都带稳定的 [`ImposeError::code`]，前端按 code 决定提示方式。

use std::fmt;

/// 核心库统一错误。
#[derive(Debug)]
pub enum ImposeError {
    /// 文件读写失败（尽量带上路径）
    Io {
        path: String,
        source: std::io::Error,
    },
    /// PDF 无法解析 / 结构损坏
    Pdf(String),
    /// 渲染、字体、图像编码失败
    Render(String),
    /// 调用参数不合法（裁剪过大、没有 SKU 等）
    Invalid(String),
    /// 数据库错误
    Db(String),
    /// 其它未归类错误
    Other(String),
}

pub type ImposeResult<T> = Result<T, ImposeError>;

impl ImposeError {
    /// 稳定错误码：前端按它分支处理，不要依赖中文文案。
    pub fn code(&self) -> &'static str {
        match self {
            ImposeError::Io { .. } => "io",
            ImposeError::Pdf(_) => "pdf",
            ImposeError::Render(_) => "render",
            ImposeError::Invalid(_) => "invalid",
            ImposeError::Db(_) => "db",
            ImposeError::Other(_) => "other",
        }
    }

    pub fn io(path: impl Into<String>, source: std::io::Error) -> Self {
        ImposeError::Io { path: path.into(), source }
    }

    pub fn pdf(msg: impl Into<String>) -> Self {
        ImposeError::Pdf(msg.into())
    }

    pub fn render(msg: impl Into<String>) -> Self {
        ImposeError::Render(msg.into())
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        ImposeError::Invalid(msg.into())
    }

    pub fn db(msg: impl Into<String>) -> Self {
        ImposeError::Db(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        ImposeError::Other(msg.into())
    }
}

impl fmt::Display for ImposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImposeError::Io { path, source } if path.is_empty() => {
                write!(f, "文件读写失败：{source}")
            }
            ImposeError::Io { path, source } => write!(f, "文件读写失败 {path}：{source}"),
            ImposeError::Pdf(msg) => write!(f, "PDF 无法解析：{msg}"),
            ImposeError::Render(msg) => write!(f, "渲染失败：{msg}"),
            ImposeError::Invalid(msg) => write!(f, "参数不合法：{msg}"),
            ImposeError::Db(msg) => write!(f, "数据库错误：{msg}"),
            // 这条已是完整句子（历史文案），不再加前缀
            ImposeError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ImposeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ImposeError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

// ---------- 外部错误转换 ----------

impl From<std::io::Error> for ImposeError {
    fn from(source: std::io::Error) -> Self {
        ImposeError::Io { path: String::new(), source }
    }
}

impl From<lopdf::Error> for ImposeError {
    fn from(e: lopdf::Error) -> Self {
        ImposeError::Pdf(e.to_string())
    }
}

impl From<pdfium_render::prelude::PdfiumError> for ImposeError {
    fn from(e: pdfium_render::prelude::PdfiumError) -> Self {
        ImposeError::Render(e.to_string())
    }
}

impl From<genpdf::error::Error> for ImposeError {
    fn from(e: genpdf::error::Error) -> Self {
        ImposeError::Render(e.to_string())
    }
}

impl From<image::ImageError> for ImposeError {
    fn from(e: image::ImageError) -> Self {
        ImposeError::Render(e.to_string())
    }
}

impl From<rusqlite::Error> for ImposeError {
    fn from(e: rusqlite::Error) -> Self {
        ImposeError::Db(e.to_string())
    }
}

impl From<serde_json::Error> for ImposeError {
    fn from(e: serde_json::Error) -> Self {
        ImposeError::Other(e.to_string())
    }
}

impl From<String> for ImposeError {
    fn from(msg: String) -> Self {
        ImposeError::Other(msg)
    }
}

impl From<&str> for ImposeError {
    fn from(msg: &str) -> Self {
        ImposeError::Other(msg.to_string())
    }
}
