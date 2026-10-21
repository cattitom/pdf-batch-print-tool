//! 单元测试专用：定位仓库内的测试 PDF。
//!
//! 之前 fixture 指向项目外另一个目录（`D:\Users\Desktop\test\pdf-tool\tests\fixtures`），
//! 换台机器就全挂。现在固定放在 `core/tests/fixtures/`，用 `CARGO_MANIFEST_DIR`
//! 拼绝对路径，与工作目录无关。

/// 读取 `core/tests/fixtures/<name>`。
///
/// 缺失时直接 panic 并给出完整路径——测试环境问题应当立刻暴露。
pub fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read(&path)
        .unwrap_or_else(|e| panic!("读取测试 fixture 失败 {}: {e}", path.display()))
}

/// fixture 文件是否存在（少数测试需要跳过而不是失败）。
pub fn has_fixture(name: &str) -> bool {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
        .is_file()
}
