//! 从 WPS 云表格同步（出货清单）：借已登录的 Chrome 取 .xls，Rust 直接解析成待办。
//!
//! 链路：带登录态的 Chrome（`kdocs-login.bat` 起的，9222 调试端口；没开就自己起 headless）
//!   → 页面内 fetch 拿 presigned 下载地址 → 下载 .xls
//!   → `core::xls` 自己解 BIFF8 → `parse_table` 变成待办（不依赖 Python）
//!
//! 取数脚本用 `include_str!` 内嵌，运行期写到缓存目录，打包时不用再配 resources。

use pdfimpose_core as core;
use serde::Serialize;

use super::{ApiError, PackingSheetDto};

const KDOCS_PULL_MJS: &str = include_str!("../../../../tools/kdocs/kdocs-pull.mjs");

/// 一次同步的结果：解析好的待办 + 这次用的表（前端拿 sheets 做表名下拉）。
#[derive(Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct KdocsSyncDto {
    pub packing: PackingSheetDto,
    /// 这次解析的表名（自动挑的话是脚本挑出来的那张）
    pub sheet: String,
    /// 这本表里所有表名，给下拉换表用
    pub sheets: Vec<String>,
    #[ts(type = "number")]
    pub bytes: u64,
    /// 这次真的重新下载了吗（fver 没变就是 false，前端自动同步可静默跳过）
    pub changed: bool,
    /// 云端版本号（fver）：排查「到底有没有更新」用
    pub fver: String,
    /// 下载下来的 .xls 缓存路径（程序启动时直接解析它，表格立刻有内容）
    pub path: String,
}

fn kdocs_workdir() -> std::path::PathBuf {
    // 优先放在程序自己旁边：整包拷到公司电脑上，缓存跟着走（绿色版）
    // 依次试 exe 目录 → 当前目录 → LOCALAPPDATA → 临时目录，取第一个真能写的
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        candidates.push(dir.join("data").join("kdocs"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("data").join("kdocs"));
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            std::path::PathBuf::from(la)
                .join("pdf-batch-print-tool")
                .join("kdocs"),
        );
    }
    candidates.push(std::env::temp_dir().join("pdf-batch-print-tool").join("kdocs"));

    for base in &candidates {
        if std::fs::create_dir_all(base.join("tools")).is_err() {
            continue;
        }
        // 目录能建不代表能写（比如带 DENY ACL 的目录），真写一个探针文件
        let probe = base.join(".writable");
        match std::fs::write(&probe, b"ok") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                return base.clone();
            }
            Err(_) => continue,
        }
    }
    // 全部失败时用它兜底（调用方的报错会带上路径，便于排查）
    candidates
        .pop()
        .unwrap_or_else(|| std::env::temp_dir().join("pdf-batch-print-tool").join("kdocs"))
}

/// 脚本内容变了才写，省得每次同步都碰磁盘。
fn write_script(path: &std::path::Path, body: &str) -> Result<(), ApiError> {
    let stale = std::fs::read_to_string(path).map(|s| s != body).unwrap_or(true);
    if stale {
        std::fs::write(path, body).map_err(|e| {
            ApiError::new("kdocs", format!("写脚本失败 {}：{e}", path.display()))
        })?;
    }
    Ok(())
}

struct Ran {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// 跑一个外部命令，带超时（超时就 kill）。stderr 原样带回给前端看。
fn spawn_bounded(
    program: &str,
    args: &[std::ffi::OsString],
    timeout: std::time::Duration,
) -> Result<Ran, ApiError> {
    use std::io::Read as _;
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 脚本里要打印中文
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW：别弹黑框
    }

    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ApiError::new("notfound", format!("找不到 {program}")),
        _ => ApiError::new("kdocs", format!("启动 {program} 失败：{e}")),
    })?;

    // 先把管道交给线程读：脚本输出一多，父子互相等管道就会假死到超时
    let out_thread = child.stdout.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = s.read_to_string(&mut buf);
            buf
        })
    });
    let err_thread = child.stderr.take().map(|mut s| {
        std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = s.read_to_string(&mut buf);
            buf
        })
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ApiError::new(
                        "timeout",
                        format!("{program} 超时（{} 秒没结束）", timeout.as_secs()),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
            Err(e) => return Err(ApiError::new("kdocs", format!("等待 {program} 失败：{e}"))),
        }
    };

    let out = out_thread.and_then(|h| h.join().ok()).unwrap_or_default();
    let err = err_thread.and_then(|h| h.join().ok()).unwrap_or_default();
    Ok(Ran { ok: status.success(), stdout: out, stderr: err })
}

/// 从 WPS 云表格拉最新来货登记，直接变成出货待办。
///
/// `sheet` 传 None 表示自动挑最新一张来货登记表；传表名只重解析那一张（复用 10 分钟内的文件）。
#[tauri::command]
pub fn sync_kdocs_packing(sheet: Option<String>) -> Result<KdocsSyncDto, ApiError> {
    let dir = kdocs_workdir();
    let tools = dir.join("tools");
    let pull_js = tools.join("kdocs-pull.mjs");
    write_script(&pull_js, KDOCS_PULL_MJS)?;

    let xls = dir.join("kdocs-latest.xls");

    // 只是换一张表时不用重新下载：10 分钟内的同一份文件直接复用
    let reuse = sheet.is_some()
        && std::fs::metadata(&xls)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|d| d.as_secs() < 600)
            .unwrap_or(false);

    let mut changed = true;
    let mut fver = String::new();
    if !reuse {
        log::info!("云表格同步：开始拉取");
        let ran = spawn_bounded(
            "node",
            &[pull_js.clone().into_os_string(), xls.clone().into_os_string()],
            std::time::Duration::from_secs(120),
        )
        .map_err(|e| {
            if e.code == "notfound" {
                ApiError::new("kdocs", "找不到 node：取数脚本要 Node.js 18 以上")
            } else {
                e
            }
        })?;
        if !ran.ok {
            return Err(ApiError::new("kdocs", ran.stderr.trim().to_string()));
        }
        let pulled: serde_json::Value =
            serde_json::from_str(ran.stdout.trim()).unwrap_or(serde_json::Value::Null);
        changed = !pulled["skipped"].as_bool().unwrap_or(false);
        fver = match &pulled["fver"] {
            serde_json::Value::String(s) => s.clone(),
            other => other.as_u64().map(|n| n.to_string()).unwrap_or_default(),
        };
        log::info!(
            "云表格同步：{}（fver={fver}）",
            if changed { "下载完成" } else { "版本没变，用本地文件" }
        );
    }

    // 直接用 Rust 解 .xls：传了表名就按表名挑，没传就自动挑最新的来货登记表
    let path = xls.to_string_lossy().to_string();
    let (packing, sheet_name, sheets) = core::parse_xls_file(&path, sheet.as_deref())
        .map_err(|e| ApiError::new(e.code(), e.to_string()))?;
    let bytes = std::fs::metadata(&xls).map(|m| m.len()).unwrap_or(0);
    log::info!(
        "云表格同步：表 {sheet_name} → {} 条待办",
        packing.items.len()
    );

    Ok(KdocsSyncDto {
        packing: packing.into(),
        sheet: sheet_name,
        sheets,
        bytes,
        changed,
        fver,
        path,
    })
}
