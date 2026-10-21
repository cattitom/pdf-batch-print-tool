//! 系统中文字体定位与缓存。
//!
//! 之前分隔页每生成一张都要 `fs::read` 一次 `msyh.ttc`（19.7MB）再解析，
//! 60 个文件就是 60 次 ~1.2GB 的无效磁盘+解析开销。字体字节只读一次，
//! 之后全局共享（含「找不到」的负缓存，避免反复试探路径）。

use std::sync::OnceLock;

/// Windows / Linux 常见中文字体路径，按优先级排列。
const CANDIDATES: [&str; 5] = [
    r"C:\Windows\Fonts\msyh.ttc",
    r"C:\Windows\Fonts\simhei.ttf",
    r"C:\Windows\Fonts\simsun.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
];

static FONT: OnceLock<Option<(String, Vec<u8>)>> = OnceLock::new();

fn load() -> Option<(String, Vec<u8>)> {
    CANDIDATES
        .iter()
        .find_map(|path| std::fs::read(path).ok().map(|bytes| ((*path).to_string(), bytes)))
}

/// 首个可用的中文字体（路径 + 字节），全局只读取一次。
pub fn cjk_font() -> Option<&'static (String, Vec<u8>)> {
    FONT.get_or_init(load).as_ref()
}

/// 候选字体路径表（按优先级）。
///
/// 不同库解析能力不同：pdfium 能吃 `.ttc` 字体集合，genpdf/rustybuzz 只能吃
/// 单字体文件，所以需要「候选表」而不只是一个结果——各自挑第一个能用的。
pub fn candidate_paths() -> &'static [&'static str] {
    &CANDIDATES
}

/// 字体字节；未找到时为 `None`。
pub fn cjk_font_bytes() -> Option<&'static [u8]> {
    cjk_font().map(|(_, bytes)| bytes.as_slice())
}
