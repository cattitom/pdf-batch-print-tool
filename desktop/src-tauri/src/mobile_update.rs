//! 手机页面更新：配对后读取整包，固定文件白名单，不接受客户端文件路径。
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const BUILTIN: &str = include_str!("../../../ios-shell/web/builtin-update.json");
const MAX_PAYLOAD: usize = 1024 * 1024;
const FILES: [&str; 5] = ["app.html", "shipping/index.html", "shipping/m.css", "shipping/m.js", "shipping/transport.js"];

#[derive(Serialize, Deserialize)]
pub struct Bundle { pub version: String, pub payload: String }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Package { schema: u32, shell_api: u32, files: std::collections::HashMap<String, String> }

fn parse(text: &str) -> Result<(Bundle, u32), String> {
    let bundle: Bundle = serde_json::from_str(text).map_err(|_| "更新文件格式不正确")?;
    if bundle.payload.len() > MAX_PAYLOAD || bundle.version != format!("{:x}", Sha256::digest(bundle.payload.as_bytes())) {
        return Err("更新文件校验失败".into());
    }
    let pkg: Package = serde_json::from_str(&bundle.payload).map_err(|_| "更新内容格式不正确")?;
    if pkg.schema != 1 || pkg.shell_api == 0 || pkg.files.len() != FILES.len()
        || FILES.iter().any(|name| pkg.files.get(*name).is_none_or(|s| s.trim().is_empty())) {
        return Err("更新文件不完整或协议不兼容".into());
    }
    Ok((bundle, pkg.shell_api))
}

fn read_file(path: &Path) -> Result<String, String> {
    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > (MAX_PAYLOAD * 2) as u64 { return Err("更新文件过大".into()); }
    std::fs::read_to_string(path).map_err(|e| format!("读取更新文件失败：{e}"))
}
fn path() -> Option<PathBuf> {
    let beside_exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("data/mobile-app/update.json")));
    if let Some(p) = beside_exe.filter(|p| p.exists()) { return Some(p); }
    if cfg!(debug_assertions) {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/mobile-app/update.json");
        if p.exists() { return Some(p); }
    }
    None
}
pub fn response(version: Option<&str>, manifest: bool) -> Result<String, (u16, String)> {
    let text = match path() { Some(p) => read_file(&p), None => Ok(BUILTIN.to_owned()) }.map_err(|e| (503, e))?;
    response_for(&text, version, manifest)
}
fn response_for(text: &str, version: Option<&str>, manifest: bool) -> Result<String, (u16, String)> {
    let (bundle, shell_api) = parse(text).map_err(|e| (503,e))?;
    if manifest {
        return Ok(serde_json::json!({"schema":1,"shellApi":shell_api,"version":bundle.version,"bytes":bundle.payload.len()}).to_string());
    }
    if version != Some(bundle.version.as_str()) { return Err((409,"页面版本已变化，请重新检查更新".into())); }
    serde_json::to_string(&bundle).map_err(|e| (500,e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builtin_integrity_and_pinned_version() {
        let (bundle, _) = parse(BUILTIN).unwrap();
        assert!(response_for(BUILTIN, None, true).unwrap().contains(&bundle.version));
        assert!(response_for(BUILTIN, Some(&bundle.version), false).is_ok());
        assert_eq!(response_for(BUILTIN, Some("stale"), false).unwrap_err().0, 409);
    }
    #[test]
    fn rejects_corrupt_or_incomplete_package() {
        let (mut bundle, _) = parse(BUILTIN).unwrap();
        bundle.payload.push(' ');
        assert!(parse(&serde_json::to_string(&bundle).unwrap()).is_err());
        bundle.payload = r#"{"schema":1,"shellApi":1,"files":{"../bad":"bad"}}"#.into();
        bundle.version = format!("{:x}", Sha256::digest(bundle.payload.as_bytes()));
        assert!(parse(&serde_json::to_string(&bundle).unwrap()).is_err());
    }
}
