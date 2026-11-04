//! 系统中文字体定位与缓存。
//!
//! 之前分隔页每生成一张都要 `fs::read` 一次 `msyh.ttc`（19.7MB）再解析，
//! 60 个文件就是 60 次 ~1.2GB 的无效磁盘+解析开销。字体字节只读一次，
//! 之后全局共享（含「找不到」的负缓存，避免反复试探路径）。
//!
//! 分隔页改成矢量描边后，还需要一个「能解析成 `ttf_parser::Face`」的字体：
//! 主字体（通常是 `msyh.ttc`，index 0 即微软雅黑常规体）解析失败时，
//! 才去读候选表里的其它字体，并同样做负缓存。

use std::sync::OnceLock;

use ttf_parser::Face;

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

// ---------- 矢量描边用字体（ttf-parser） ----------

/// 解析好的轮廓字体。
///
/// `Face` 借自 `cjk_font()` / 备用字体缓存里的 `&'static [u8]`，
/// 字体字节本身活到进程结束，所以可以把 `Face<'static>` 全局缓存下来，
/// 不必每张分隔页重新解析。
pub struct OutlineFont {
    /// 命中的字体路径（日志/排查用）。
    pub path: String,
    face: Face<'static>,
}

impl OutlineFont {
    /// 字体面（只读）。
    pub fn face(&self) -> &Face<'static> {
        &self.face
    }
}

/// 备用字体字节缓存：只有主字体解析失败时才读盘，且只读一次。
static OUTLINE_FALLBACK: OnceLock<Option<(String, Vec<u8>)>> = OnceLock::new();

static OUTLINE_FONT: OnceLock<Option<OutlineFont>> = OnceLock::new();

fn load_fallback() -> Option<(String, Vec<u8>)> {
    let primary = cjk_font().map(|(path, _)| path.as_str());
    for path in candidate_paths() {
        if Some(*path) == primary {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        match Face::parse(&bytes, 0) {
            Ok(_) => return Some(((*path).to_string(), bytes)),
            Err(e) => log::warn!("备用字体 {path} 无法解析：{e}"),
        }
    }
    None
}

fn load_outline_font() -> Option<OutlineFont> {
    // 1) 主字体：msyh.ttc 是字体集合，index 0 = 微软雅黑常规体
    if let Some((path, bytes)) = cjk_font() {
        match Face::parse(bytes.as_slice(), 0) {
            Ok(face) => return Some(OutlineFont { path: path.clone(), face }),
            Err(e) => log::warn!("主字体 {path} 无法解析（{e}），尝试备用字体"),
        }
    }
    // 2) 候选表里其它能解析的字体（simhei.ttf 等）
    let (path, bytes) = OUTLINE_FALLBACK.get_or_init(load_fallback).as_ref()?;
    match Face::parse(bytes.as_slice(), 0) {
        Ok(face) => {
            log::warn!("分隔页矢量文字改用备用字体 {path}");
            Some(OutlineFont { path: path.clone(), face })
        }
        Err(e) => {
            log::warn!("备用字体 {path} 也无法解析：{e}");
            None
        }
    }
}

/// 首个可用于矢量描边的字体（含「无可用字体」负缓存，全局只解析一次）。
pub fn outline_font() -> Option<&'static OutlineFont> {
    OUTLINE_FONT.get_or_init(load_outline_font).as_ref()
}
