//! 微信消息标签页的 Tauri 命令：扫描解密 / 群列表 / 消息 / 导出 CSV。
//!
//! 扫描要读进程内存、解密整个 db_storage（几十秒起步），全部丢 spawn_blocking，
//! 别占 Tauri 主线程（cd 之前 kdocs 同步就踩过这个坑）。

use std::path::PathBuf;

use super::ApiError;
use crate::wechat;

fn wrap(e: String) -> ApiError {
    ApiError::new("wechat", e)
}

#[tauri::command]
pub fn wechat_status(extra: Option<Vec<String>>) -> wechat::StatusDto {
    wechat::status(&extra.unwrap_or_default())
}

/// 校验用户手选的目录（xwechat_files 根 / wxid_* 账号目录 / db_storage 都行）。
#[tauri::command]
pub fn wechat_check_dir(path: String) -> Vec<wechat::AccountDto> {
    wechat::check_dir(&path)
}

#[tauri::command]
pub async fn wechat_scan(account: String) -> Result<wechat::ScanDto, ApiError> {
    tauri::async_runtime::spawn_blocking(move || wechat::scan(&PathBuf::from(account)))
        .await
        .map_err(|e| ApiError::new("wechat", format!("扫描任务异常结束：{e}")))?
        .map_err(wrap)
}

#[tauri::command]
pub async fn wechat_rooms(account: String) -> Result<Vec<wechat::RoomDto>, ApiError> {
    tauri::async_runtime::spawn_blocking(move || wechat::rooms(&PathBuf::from(account)))
        .await
        .map_err(|e| ApiError::new("wechat", format!("读取群列表异常结束：{e}")))?
        .map_err(wrap)
}

#[tauri::command]
pub async fn wechat_messages(
    account: String,
    chat_id: String,
    days: Option<i64>,
    limit: Option<usize>,
    kind: Option<u32>,
) -> Result<Vec<wechat::MsgDto>, ApiError> {
    tauri::async_runtime::spawn_blocking(move || {
        wechat::messages(&PathBuf::from(account), &chat_id, days, limit, kind)
    })
    .await
    .map_err(|e| ApiError::new("wechat", format!("读取消息异常结束：{e}")))?
    .map_err(wrap)
}

/// 监控轮询：补 WAL 后取 `after_ts` 之后的新消息。
#[tauri::command]
pub async fn wechat_poll(
    account: String,
    chat_id: String,
    after_ts: i64,
) -> Result<Vec<wechat::MsgDto>, ApiError> {
    tauri::async_runtime::spawn_blocking(move || {
        wechat::poll(&PathBuf::from(account), &chat_id, after_ts)
    })
    .await
    .map_err(|e| ApiError::new("wechat", format!("监控轮询异常结束：{e}")))?
    .map_err(wrap)
}

#[tauri::command]
pub async fn wechat_export_csv(
    account: String,
    chat_id: String,
    days: Option<i64>,
    kind: Option<u32>,
    path: String,
) -> Result<wechat::ExportDto, ApiError> {
    tauri::async_runtime::spawn_blocking(move || {
        wechat::export_csv(
            &PathBuf::from(account),
            &chat_id,
            days,
            kind,
            &PathBuf::from(path),
        )
    })
    .await
    .map_err(|e| ApiError::new("wechat", format!("导出任务异常结束：{e}")))?
    .map_err(wrap)
}
