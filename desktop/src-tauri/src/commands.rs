//! Tauri commands：前端 ↔ pdfimpose-core 的桥接层。
//!
//! 设计要点：
//! - 文件一律传**路径**，由 Rust 端读取字节，避免大文件走 IPC
//! - 拼版输出直接写盘（前端先用 dialog 选好保存路径），只回传路径与统计
//! - 进度通过事件 `impose-progress` 推送（done, total）
//! - Db 用 Mutex 包住（rusqlite Connection 非 Sync）

use base64::Engine as _;
use pdfimpose_core as core;
use crate::mobile::{self, MobileInfo, MobileRow, MobileShared};
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

// ---------- 状态 ----------

pub struct AppState {
    pub db: Mutex<core::Db>,
}

/// 命令错误：前端只依赖 `code`，`message` 用于直接展示。
#[derive(Debug, Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/bindings/")]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into() }
    }
}

impl From<core::ImposeError> for ApiError {
    fn from(e: core::ImposeError) -> Self {
        Self::new(e.code(), e.to_string())
    }
}


impl From<core::rusqlite::Error> for ApiError {
    fn from(e: core::rusqlite::Error) -> Self {
        Self::new("db", e.to_string())
    }
}

impl From<String> for ApiError {
    fn from(message: String) -> Self {
        Self::new("other", message)
    }
}

/// 初始化应用状态（数据库放在系统 appdata 目录）。
pub fn init_state(app: &tauri::App) -> Result<AppState, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法定位应用数据目录：{e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建应用数据目录：{e}"))?;
    let db = core::Db::open(dir.join("pdf-tool.db"))
        .map_err(|e| format!("打开数据库失败：{e}"))?;
    Ok(AppState { db: Mutex::new(db) })
}

fn db<'a>(state: &'a State<AppState>) -> std::sync::MutexGuard<'a, core::Db> {
    state.db.lock().expect("数据库锁被占用")
}

// ---------- DTO ----------

/// 裁剪参数（type tag + camelCase 字段，前端好写）。
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub enum CropDto {
    #[serde(rename_all = "camelCase")]
    Auto { margin_mm: f64 },
    #[serde(rename_all = "camelCase")]
    Rect { x0: f64, y0: f64, x1: f64, y1: f64 },
    #[serde(rename_all = "camelCase")]
    Manual { top_mm: f64, bottom_mm: f64, left_mm: f64, right_mm: f64 },
}

impl CropDto {
    fn into_core(self) -> core::CropSpec {
        match self {
            CropDto::Auto { margin_mm } => core::CropSpec::Auto { margin_mm },
            CropDto::Rect { x0, y0, x1, y1 } => core::CropSpec::Rect { x0, y0, x1, y1 },
            CropDto::Manual { top_mm, bottom_mm, left_mm, right_mm } => core::CropSpec::Manual {
                top_mm,
                bottom_mm,
                left_mm,
                right_mm,
            },
        }
    }
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct ImposeFileDto {
    /// 展示名（含扩展名）
    pub name: String,
    /// 完整路径，Rust 端读字节
    pub path: String,
    pub target_pages: u32,
    #[ts(type = "number")]
    pub rotate: i64,
    pub crop: Option<CropDto>,
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct ImposeOptsDto {
    pub sep_pages: u32,
    pub sep_text: Option<String>,
    /// 每个文件的内容页盖页码「文件号-页/该文件页数」
    pub page_numbers: bool,
}

#[derive(Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct ImposeResultDto {
    pub path: String,
    pub pages: u32,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    #[ts(type = "number")]
    pub size_bytes: u64,
}

/// 自动打开成品的结果：app 用于界面提示（WPS PDF / WPS Office）。
#[derive(Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct OpenPdfResultDto {
    pub app: String,
    pub exe: String,
}

// ---------- 工具 ----------

fn read_pdf(path: &str) -> Result<Vec<u8>, core::ImposeError> {
    std::fs::read(path).map_err(|e| core::ImposeError::io(path, e))
}

// ---------- PDF 信息 ----------

#[tauri::command]
pub fn pdf_infos(paths: Vec<String>) -> Result<Vec<core::PdfInfo>, ApiError> {
    paths
        .iter()
        .map(|p| core::pdf_info(&read_pdf(p)?).map_err(ApiError::from))
        .collect()
}

/// 渲染 PDF 前 `maxPages` 页为 base64 PNG 数组（预览）。
#[tauri::command]
pub fn preview_pdf(path: String, dpi: f32, max_pages: u32) -> Result<Vec<String>, ApiError> {
    let bytes = read_pdf(&path)?;
    let pngs = core::render_pages(&bytes, dpi, max_pages).map_err(ApiError::from)?;
    Ok(pngs
        .into_iter()
        .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
        .collect())
}

// ---------- 拼版 ----------

#[tauri::command]
pub fn impose_pdfs(
    app: AppHandle,
    files: Vec<ImposeFileDto>,
    opts: ImposeOptsDto,
    out_path: String,
) -> Result<ImposeResultDto, ApiError> {
    // 只传路径：真正读盘发生在 core 里，读完即释放（峰值内存≈单个文件）
    let inputs: Vec<core::FileInput> = files
        .iter()
        .map(|f| {
            let mut input = core::FileInput::from_path(f.name.clone(), f.path.clone());
            input.target_pages = f.target_pages;
            input.rotate = f.rotate;
            input.crop = f.crop.clone().map(|c| c.into_core());
            input
        })
        .collect();

    let core_opts = core::ImposeOptions {
        sep_pages: opts.sep_pages,
        sep_text: opts.sep_text,
        page_numbers: opts.page_numbers,
    };

    log::info!(
        "拼版开始：{} 个文件，分隔页 {}，页码 {}",
        inputs.len(),
        core_opts.sep_pages,
        core_opts.page_numbers
    );
    let t0 = std::time::Instant::now();

    // 重名保护：目标已存在（含与源文件同名）时自动加 " (n)" 后缀，绝不覆盖
    let mut final_path = std::path::PathBuf::from(&out_path);
    if final_path.exists() {
        let dir = final_path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
        let stem = final_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output")
            .to_string();
        let ext = final_path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| format!(".{s}"))
            .unwrap_or_default();
        let mut n = 1;
        loop {
            let cand = dir.join(format!("{stem} ({n}){ext}"));
            if !cand.exists() {
                final_path = cand;
                break;
            }
            n += 1;
        }
    }
    let final_path_str = final_path.to_string_lossy().to_string();

    // 流式写盘：不把整份成品留在内存里；失败时删掉半截文件
    let file = std::fs::File::create(&final_path)
        .map_err(|e| ApiError::from(core::ImposeError::io(&final_path_str, e)))?;
    let mut writer = std::io::BufWriter::new(file);
    let stats = match core::impose_to(&inputs, &core_opts, &|done, total| {
        let _ = app.emit("impose-progress", serde_json::json!({ "done": done, "total": total }));
    }, &mut writer) {
        Ok(s) => s,
        Err(e) => {
            log::error!("拼版失败：{e}");
            drop(writer);
            let _ = std::fs::remove_file(&final_path);
            return Err(ApiError::from(e));
        }
    };
    writer
        .flush()
        .map_err(|e| ApiError::from(core::ImposeError::io(&final_path_str, e)))?;
    drop(writer);

    log::info!(
        "拼版完成：{} 页，{:?} → {}",
        stats.pages,
        stats.elapsed,
        final_path_str
    );
    Ok(ImposeResultDto {
        path: final_path_str,
        pages: stats.pages as u32,
        elapsed_ms: t0.elapsed().as_millis() as u64,
        size_bytes: std::fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0),
    })
}

// ---------- 自动打开 ----------

/// 合成完成后用 WPS PDF 打开成品。
///
/// 不依赖系统默认 PDF 关联（本机可能仍关联 Edge），而是直接找 WPS 的
/// `wpspdf.exe` 并用它启动文件；找不到 WPS 时返回 `wps_not_found`。
#[tauri::command]
pub fn open_pdf_in_wps(path: String) -> Result<OpenPdfResultDto, ApiError> {
    if !std::path::Path::new(&path).is_file() {
        return Err(ApiError::new("io", format!("找不到要打开的文件：{path}")));
    }
    let exe = find_wps_exe()
        .ok_or_else(|| ApiError::new("wps_not_found", "未检测到 WPS Office，请确认已安装 WPS"))?;
    launch_detached(&exe, &path)
        .map_err(|e| ApiError::new("io", format!("启动 WPS 失败：{e}")))?;
    log::info!("已用 WPS 打开成品：{path}（{}）", exe.display());

    let is_pdf = exe
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("wpspdf.exe"));
    Ok(OpenPdfResultDto {
        app: if is_pdf { "WPS PDF" } else { "WPS Office" }.to_string(),
        exe: exe.to_string_lossy().to_string(),
    })
}

#[cfg(windows)]
fn launch_detached(exe: &std::path::Path, path: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    // GUI 程序独立运行，不等它退出；CREATE_NO_WINDOW 避免闪控制台
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(exe)
        .arg(path)
        .current_dir(exe.parent().unwrap_or_else(|| std::path::Path::new(".")))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
}

#[cfg(not(windows))]
fn launch_detached(_exe: &std::path::Path, _path: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "当前系统不支持直接调用 WPS",
    ))
}

#[cfg(not(windows))]
fn find_wps_exe() -> Option<std::path::PathBuf> {
    None
}

/// 依次从 WPS 注册表安装目录、常见安装位置、App Paths 查找可执行文件。
#[cfg(windows)]
fn find_wps_exe() -> Option<std::path::PathBuf> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};

    let mut roots: Vec<std::path::PathBuf> = Vec::new();

    // 1) WPS 自己登记的安装目录（装在非系统盘也能找到，最可靠）
    for (hive, sub) in [
        (HKEY_CURRENT_USER, r"Software\Kingsoft\Office\6.0\common"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Kingsoft\Office\6.0\common"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Kingsoft\Office\6.0\common"),
    ] {
        if let Ok(key) = RegKey::predef(hive).open_subkey_with_flags(sub, KEY_READ)
            && let Ok(raw) = key.get_value::<String, _>("InstallRoot")
        {
            let raw = raw.trim().trim_matches('"');
            if !raw.is_empty() {
                roots.push(std::path::PathBuf::from(raw));
            }
        }
    }

    // 2) 常见安装位置兜底
    for env in [
        "LOCALAPPDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PROGRAMW6432",
    ] {
        if let Ok(base) = std::env::var(env) {
            roots.push(
                std::path::PathBuf::from(base)
                    .join("Kingsoft")
                    .join("WPS Office"),
            );
        }
    }

    // 3) App Paths 只登记 wps.exe 的机器：同目录一般也有 wpspdf.exe
    for (hive, sub) in [
        (
            HKEY_CURRENT_USER,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\wpspdf.exe",
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\wpspdf.exe",
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths\wpspdf.exe",
        ),
        (
            HKEY_CURRENT_USER,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\wps.exe",
        ),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\wps.exe",
        ),
    ] {
        if let Ok(key) = RegKey::predef(hive).open_subkey_with_flags(sub, KEY_READ)
            && let Ok(raw) = key.get_value::<String, _>("")
        {
            let raw = raw.trim().trim_matches('"');
            if !raw.is_empty() {
                let exe = std::path::PathBuf::from(raw);
                if exe.is_file() {
                    return prefer_wps_pdf(&exe);
                }
            }
        }
    }

    roots.into_iter().find_map(|root| find_in_wps_root(&root))
}

/// 优先用 WPS PDF（wpspdf.exe）；只有 wps.exe 时退回 WPS Office。
#[cfg(windows)]
fn prefer_wps_pdf(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let is_wps = exe
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("wps.exe"));
    if is_wps {
        let pdf = exe.with_file_name("wpspdf.exe");
        if pdf.is_file() {
            return Some(pdf);
        }
    }
    Some(exe.to_path_buf())
}

#[cfg(windows)]
fn find_in_wps_root(root: &std::path::Path) -> Option<std::path::PathBuf> {
    // InstallRoot 通常直接是版本目录（...\12.1.0.xxxxx），下面有 office6
    if let Some(exe) = wps_exe_in(root) {
        return Some(exe);
    }
    // 兜底路径是 ...\WPS Office：遍历版本子目录，新版本优先
    let mut versions: Vec<std::path::PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    versions.sort();
    versions.reverse();
    versions.into_iter().find_map(|dir| wps_exe_in(&dir))
}

#[cfg(windows)]
fn wps_exe_in(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    for rel in [
        r"office6\wpspdf.exe",
        "wpspdf.exe",
        r"office6\wps.exe",
        "wps.exe",
    ] {
        let exe = dir.join(rel);
        if exe.is_file() {
            return Some(exe);
        }
    }
    None
}

/// 日志文件路径（设置页「打开日志目录」用）。
#[tauri::command]
pub fn log_dir() -> String {
    crate::logging::log_path().to_string_lossy().to_string()
}

// ---------- 打印历史 ----------

#[tauri::command]
pub fn add_print_history(
    state: State<AppState>,
    entry: core::PrintHistoryInput,
) -> Result<i64, ApiError> {
    db(&state)
        .add_print_history(&entry)
        .map_err(ApiError::from)
}

#[tauri::command]
pub fn list_print_history(
    state: State<AppState>,
    limit: u32,
    offset: u32,
) -> Result<Vec<core::PrintHistoryEntry>, ApiError> {
    db(&state)
        .list_print_history(limit, offset)
        .map_err(ApiError::from)
}

#[tauri::command]
pub fn delete_print_history(state: State<AppState>, id: i64) -> Result<(), ApiError> {
    db(&state).delete_print_history(id).map(|_| ()).map_err(ApiError::from)
}

#[tauri::command]
pub fn clear_print_history(state: State<AppState>) -> Result<(), ApiError> {
    db(&state).clear_print_history().map(|_| ()).map_err(ApiError::from)
}

// ---------- 箱唛 ----------

#[derive(Debug, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct LabelSpecDto {
    pub inbound_no: String,
    pub warehouse: String,
    pub client: String,
    pub shelf: String,
    pub skus: Vec<core::LabelSku>,
}

impl LabelSpecDto {
    fn into_core(self) -> Result<core::LabelDocSpec, ApiError> {
        Ok(core::LabelDocSpec {
            inbound_no: self.inbound_no,
            warehouse: self.warehouse,
            client: self.client,
            shelf: self.shelf,
            skus: self.skus,
        })
    }
}

#[derive(Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct LabelResultDto {
    pub path: String,
    pub boxes: usize,
}

#[tauri::command]
pub fn generate_labels(state: State<AppState>, spec: LabelSpecDto, out_path: String) -> Result<LabelResultDto, ApiError> {
    let spec = spec.into_core()?;
    let plan = core::build_plan(&spec);
    let t0 = std::time::Instant::now();
    let bytes = match core::generate_box_labels(&spec) {
        Ok(b) => b,
        Err(e) => {
            log::error!("箱唛生成失败：{e}");
            return Err(ApiError::from(e));
        }
    };
    log::info!(
        "箱唛生成完成：{} 箱，{:?}，{} 字节",
        plan.len(),
        t0.elapsed(),
        bytes.len()
    );
    std::fs::write(&out_path, &bytes)
        .map_err(|e| ApiError::from(core::ImposeError::io(&out_path, e)))?;

    // 顺带把单据存进历史（与 Streamlit 版行为一致：生成即记录）
    let _ = db(&state).save_label_doc(&core::LabelDocInput {
        inbound_no: spec.inbound_no.clone(),
        warehouse: spec.warehouse.clone(),
        client: spec.client.clone(),
        shelf: spec.shelf.clone(),
        skus: serde_json::to_value(&spec.skus).unwrap_or_default(),
        total_boxes: plan.len() as i64,
    });

    Ok(LabelResultDto { path: out_path, boxes: plan.len() })
}

/// 生成第一页预览图（base64 PNG）。
#[tauri::command]
pub fn preview_label(spec: LabelSpecDto) -> Result<String, ApiError> {
    // 只排第一箱：UI 每次改表单都会自动来一次，不能按「生成全部」的成本算
    let spec = spec.into_core()?;
    let pdf = core::generate_label_preview(&spec).map_err(ApiError::from)?;
    let png = core::render_preview(&pdf, 96.0).map_err(ApiError::from)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(png))
}

#[tauri::command]
pub fn list_label_docs(
    state: State<AppState>,
    limit: u32,
) -> Result<Vec<core::LabelDocEntry>, ApiError> {
    db(&state).list_label_docs(limit).map_err(ApiError::from)
}

#[tauri::command]
pub fn latest_label_doc(state: State<AppState>) -> Result<Option<core::LabelDocEntry>, ApiError> {
    db(&state).latest_label_doc().map_err(ApiError::from)
}

// ---------- 产品标签（50×30mm） ----------

/// 生成结果：输出路径 + 总页数。
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/bindings/")]
pub struct ProductLabelResultDto {
    pub path: String,
    pub pages: usize,
}

/// 批量生成产品标签 PDF（一张一页）。
#[tauri::command]
pub fn generate_product_labels(
    items: Vec<core::ProductLabelItem>,
    out_path: String,
) -> Result<ProductLabelResultDto, ApiError> {
    let pages = core::total_labels(&items);
    if pages == 0 {
        return Err(ApiError::new("invalid", "请至少填写一行编码"));
    }
    let bytes = core::generate_product_labels(&items).map_err(ApiError::from)?;
    std::fs::write(&out_path, &bytes).map_err(|e| ApiError::new("io", e.to_string()))?;
    log::info!("产品标签已生成：{out_path}（{pages} 页）");
    Ok(ProductLabelResultDto { path: out_path, pages })
}

/// 首页预览（base64 PNG），只排第一行。
#[tauri::command]
pub fn preview_product_label(items: Vec<core::ProductLabelItem>) -> Result<String, ApiError> {
    let pdf = core::generate_product_label_preview(&items).map_err(ApiError::from)?;
    let png = core::render_preview(&pdf, 150.0).map_err(ApiError::from)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(png))
}

// ---------- 设置 ----------

#[tauri::command]
pub fn all_settings(state: State<AppState>) -> Result<std::collections::HashMap<String, String>, ApiError> {
    Ok(db(&state).all_settings().map_err(ApiError::from)?.into_iter().collect())
}

#[tauri::command]
pub fn set_setting(state: State<AppState>, key: String, value: String) -> Result<(), ApiError> {
    db(&state).set_setting(&key, &value).map_err(ApiError::from)
}

// ---------- 出货清单：已录勾选 ----------

/// 一条勾选记录，按「单号 + 入库号 + 商品名」认行。
#[derive(Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct PackingCheckDto {
    pub waybill: String,
    pub inbound: String,
    pub name: String,
    pub checked: bool,
}

#[tauri::command]
pub fn set_packing_check(
    state: State<AppState>,
    waybill: String,
    inbound: String,
    name: String,
    checked: bool,
) -> Result<(), ApiError> {
    db(&state)
        .set_packing_check(&waybill, &inbound, &name, checked)
        .map_err(ApiError::from)
}

#[tauri::command]
pub fn list_packing_checks(state: State<AppState>) -> Result<Vec<PackingCheckDto>, ApiError> {
    let rows = db(&state).all_packing_checks().map_err(ApiError::from)?;
    Ok(rows
        .into_iter()
        .map(|(waybill, inbound, name, checked)| PackingCheckDto {
            waybill,
            inbound,
            name,
            checked,
        })
        .collect())
}

#[tauri::command]
pub fn clear_packing_checks(state: State<AppState>) -> Result<(), ApiError> {
    db(&state).clear_packing_checks().map_err(ApiError::from)
}

// ---------- 装箱单解析 ----------

/// 一条出货待办。字段名与 core::PackingItem 一致，前端直接渲染。
#[derive(Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct PackingItemDto {
    pub seq: u32,
    pub name: String,
    #[ts(type = "number")]
    pub qty: i64,
    pub qty_raw: String,
    pub inbound: String,
    pub destination: String,
    /// 原表的「到货情况」（到货 / 未到货 / 部分到货）
    pub arrival: String,
    /// 客户 / 供应商（表里通常只在段首写一次，解析时向下沿用）
    pub customer: String,
    pub waybill: String,
    pub checked: bool,
    /// name / qty / inbound / inbound_sci / mark_text / dup_mark
    pub issues: Vec<String>,
}

#[derive(Debug, Serialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../../src/bindings/")]
pub struct PackingSheetDto {
    pub items: Vec<PackingItemDto>,
    pub destination: String,
    pub waybill: String,
    #[ts(type = "number")]
    pub total_qty: i64,
    #[ts(type = "number | null")]
    pub expected_qty: Option<i64>,
    /// 列映射说明：「按表头识别：名称=第1列 …」
    pub mapping: String,
    pub warnings: Vec<String>,
    /// 截图识别用的引擎："paddle" / "system"；粘贴和 xlsx 路径为空
    pub engine: String,
}

impl From<core::PackingSheet> for PackingSheetDto {
    fn from(s: core::PackingSheet) -> Self {
        Self {
            items: s
                .items
                .into_iter()
                .map(|i| PackingItemDto {
                    seq: i.seq,
                    name: i.name,
                    qty: i.qty,
                    qty_raw: i.qty_raw,
                    inbound: i.inbound,
                    destination: i.destination,
                    arrival: i.arrival,
                    customer: i.customer,
                    waybill: i.waybill,
                    checked: i.checked,
                    issues: i.issues,
                })
                .collect(),
            destination: s.destination,
            waybill: s.waybill,
            total_qty: s.total_qty,
            expected_qty: s.expected_qty,
            mapping: s.mapping,
            warnings: s.warnings,
            engine: String::new(),
        }
    }
}

/// 粘贴文本（Excel 直接复制 = 制表符分隔）→ 结构化待办。
#[tauri::command]
pub fn parse_packing_text(text: String) -> Result<PackingSheetDto, ApiError> {
    if text.trim().is_empty() {
        return Err(ApiError::new("empty", "先粘贴表格内容，或拖 xlsx 进来"));
    }
    Ok(core::parse_text(&text).into())
}

/// 拖进来的文件（xlsx / xls / csv / txt）→ 结构化待办。
#[tauri::command]
pub fn parse_packing_file(path: String) -> Result<PackingSheetDto, ApiError> {
    let sheet = core::parse_file(&path).map_err(|e| ApiError::new(e.code(), e.to_string()))?;
    log::info!("装箱单解析：{path} → {} 条", sheet.items.len());
    Ok(sheet.into())
}

/// 指定 .xls 里的一张表来解析（云表格缓存恢复时用：上次看的是「11月」就还是「11月」）。
#[tauri::command]
pub fn parse_packing_xls(path: String, sheet: Option<String>) -> Result<PackingSheetDto, ApiError> {
    let (parsed, picked, _) = core::parse_xls_file(&path, sheet.as_deref())
        .map_err(|e| ApiError::new(e.code(), e.to_string()))?;
    log::info!("装箱单解析：{path}（表 {picked}） → {} 条", parsed.items.len());
    Ok(parsed.into())
}

/// 图片（截屏/拍照）→ 结构化待办：走 Windows 自带 OCR，离线免费。
#[tauri::command]
pub fn parse_packing_image(path: String) -> Result<PackingSheetDto, ApiError> {
    #[cfg(windows)]
    {
        let sheet = crate::ocr::parse_image(&path).map_err(|e| ApiError::new("ocr", e))?;
        log::info!("截图识别：{path} → {} 条", sheet.items.len());
        let mut dto: PackingSheetDto = sheet.into();
        dto.engine = crate::ocr::engine_name().to_string();
        Ok(dto)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(ApiError::new("ocr", "截图识别目前只支持 Windows"))
    }
}

// ---------- 从 WPS 云表格同步（出货清单） ----------
// 实现见 commands/kdocs.rs：取数链路独立成模块，主文件只留入口。
pub mod kdocs;

// ---------- 微信消息（本地库解密/读取/导出） ----------
pub mod wechat;

/// 把生成的 Markdown 清单写盘（路径由前端保存对话框给）。
#[tauri::command]
pub fn save_text_file(path: String, content: String) -> Result<(), ApiError> {
    std::fs::write(&path, content)
        .map_err(|e| ApiError::new("io", format!("写入 {path} 失败：{e}")))
}

/// 读日志尾部（给界面里的「日志」按钮用，不用让人去找文件）。
#[tauri::command]
pub fn read_log_tail(lines: Option<u32>) -> Result<String, ApiError> {
    let path = crate::logging::log_path();
    if path.as_os_str().is_empty() {
        return Err(ApiError::new("log", "日志还没初始化"));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| ApiError::new("io", format!("读日志失败：{e}")))?;
    let n = lines.unwrap_or(400).clamp(20, 5000) as usize;
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(n);
    Ok(all[start..].join("\n"))
}

/// 清空当前日志（日志器持有的是追加句柄，这里截断同一个文件即可）。
#[tauri::command]
pub fn clear_log() -> Result<(), ApiError> {
    let path = crate::logging::log_path();
    if path.as_os_str().is_empty() {
        return Err(ApiError::new("log", "日志还没初始化"));
    }
    std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(|e| ApiError::new("io", format!("清空日志失败：{e}")))?;
    log::info!("日志已由用户在界面清空");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 锁定前后端 JSON 契约：camelCase 字段 + type tag。
    #[test]
    fn crop_dto_wire_format() {
        let auto: CropDto =
            serde_json::from_str(r#"{"type":"auto","marginMm":2.0}"#).unwrap();
        assert!(matches!(auto.clone().into_core(), core::CropSpec::Auto { margin_mm } if margin_mm == 2.0));

        let manual: CropDto = serde_json::from_str(
            r#"{"type":"manual","topMm":10,"bottomMm":5,"leftMm":8,"rightMm":8}"#,
        )
        .unwrap();
        assert!(matches!(
            manual.into_core(),
            core::CropSpec::Manual { top_mm: 10.0, .. }
        ));

        // 反序列化回 JSON 也应是 camelCase
        let rect = CropDto::Rect { x0: 1.0, y0: 2.0, x1: 3.0, y1: 4.0 };
        let json = serde_json::to_value(&rect).unwrap();
        assert_eq!(json["type"], "rect");
        assert_eq!(json["x0"], 1.0);
    }

    #[test]
    fn impose_dto_wire_format() {
        let f: ImposeFileDto = serde_json::from_value(serde_json::json!({
            "name": "a.pdf", "path": "C:/x/a.pdf",
            "targetPages": 6, "rotate": 90,
            "crop": {"type":"auto","marginMm":3}
        }))
        .unwrap();
        assert_eq!(f.target_pages, 6);
        assert_eq!(f.rotate, 90);

        let o: ImposeOptsDto = serde_json::from_value(serde_json::json!({
            "sepPages": 1, "sepText": "这是分隔线", "pageNumbers": false
        }))
        .unwrap();
        assert_eq!(o.sep_pages, 1);
    }

    /// 箱唛 SKU 之前漏了 camelCase：前端传 pcsPerBox，Rust 按 pcs_per_box 收不到，
    /// 被 serde 默认值吞成 1（每箱件数静默丢失）。这条测试钉住字段名。
    #[test]
    fn label_sku_wire_format_is_camel_case() {
        let spec: LabelSpecDto = serde_json::from_value(serde_json::json!({
            "inboundNo": "RK1", "warehouse": "W", "client": "C", "shelf": "S1",
            "skus": [{
                "sku": "X", "boxes": 2, "pcsPerBox": 7,
                "quantities": "10*2,7", "newFlag": false
            }]
        }))
        .unwrap();
        let sku = &spec.skus[0];
        assert_eq!(sku.pcs_per_box, 7, "pcsPerBox 必须进 pcs_per_box");
        assert_eq!(sku.boxes, 2);
        assert!(!sku.new_flag, "newFlag 必须进 new_flag");
        assert_eq!(sku.quantities, "10*2,7");
    }

    /// 装箱单：命令层 → 前端 JSON 契约（camelCase）。
    #[test]
    fn packing_sheet_wire_format() {
        let sheet = parse_packing_text(
            "商品\t数量\t入库号\t标记\n门铰链\t10\tA1001\t1\n鱼鳞布\t8\tA1002\t".to_string(),
        )
        .unwrap();
        assert_eq!(sheet.items.len(), 2);
        assert_eq!(sheet.items[0].qty, 10);
        assert!(sheet.items[0].checked);
        assert_eq!(sheet.total_qty, 18);

        let json = serde_json::to_value(&sheet).unwrap();
        assert!(json["totalQty"].is_number(), "totalQty 必须是 number 不是 BigInt");
        assert!(json["items"][0]["inbound"].is_string());
        assert!(json["warnings"].is_array());
    }

    #[test]
    fn packing_text_rejects_empty() {
        assert!(parse_packing_text("   ".to_string()).is_err());
    }

    /// 版本目录里应优先选 wpspdf.exe；没有时退回 wps.exe。
    #[cfg(windows)]
    #[test]
    fn wps_exe_lookup_prefers_pdf_executable() {
        let root = std::env::temp_dir().join(format!("pdf-tool-wps-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let office = root.join("12.1.0.1").join("office6");
        std::fs::create_dir_all(&office).unwrap();

        std::fs::write(office.join("wps.exe"), b"").unwrap();
        assert_eq!(wps_exe_in(&office).unwrap(), office.join("wps.exe"));

        std::fs::write(office.join("wpspdf.exe"), b"").unwrap();
        assert_eq!(wps_exe_in(&office).unwrap(), office.join("wpspdf.exe"));
        assert_eq!(find_in_wps_root(&root).unwrap(), office.join("wpspdf.exe"));

        std::fs::remove_dir_all(&root).unwrap();
    }
}
// ---------- 手机出货（局域网） ----------

/// 手机端口令：存进本机设置，重启后不变；手机存书签就不用每次扫码。
const MOBILE_TOKEN_KEY: &str = "mobile_token";
/// 上次手机服务成功监听的端口：下次优先用它，手机书签（整条 URL）才不会失效。
const MOBILE_PORT_KEY: &str = "mobile_port";

fn mobile_token(state: &State<'_, AppState>) -> Result<String, ApiError> {
    let guard = db(state);
    if let Some(t) = guard.get_setting(MOBILE_TOKEN_KEY).map_err(ApiError::from)? {
        if t.len() >= 8 {
            return Ok(t);
        }
    }
    let t = mobile::new_token();
    guard
        .set_setting(MOBILE_TOKEN_KEY, &t)
        .map_err(ApiError::from)?;
    Ok(t)
}

#[tauri::command]
pub fn mobile_start(
    state: State<'_, AppState>,
    mobile_state: State<'_, MobileShared>,
    rows: Vec<MobileRow>,
) -> Result<MobileInfo, ApiError> {
    let token = mobile_token(&state)?;
    let prefer = db(&state)
        .get_setting(MOBILE_PORT_KEY)
        .ok()
        .flatten()
        .and_then(|s| s.trim().parse::<u16>().ok());
    let info = mobile::start(&mobile_state, rows, token, prefer)
        .map_err(|e| ApiError::new("mobile", e))?;
    if info.port > 1024 {
        let _ = db(&state).set_setting(MOBILE_PORT_KEY, &info.port.to_string());
    }
    Ok(info)
}

#[tauri::command]
pub fn mobile_reset_token(
    state: State<'_, AppState>,
    mobile_state: State<'_, MobileShared>,
) -> Result<MobileInfo, ApiError> {
    let rows = mobile::rows(&mobile_state);
    mobile::stop(&mobile_state);
    db(&state)
        .set_setting(MOBILE_TOKEN_KEY, "")
        .map_err(ApiError::from)?;
    let token = mobile_token(&state)?;
    let prefer = db(&state)
        .get_setting(MOBILE_PORT_KEY)
        .ok()
        .flatten()
        .and_then(|s| s.trim().parse::<u16>().ok());
    let info = mobile::start(&mobile_state, rows, token, prefer)
        .map_err(|e| ApiError::new("mobile", e))?;
    if info.port > 1024 {
        let _ = db(&state).set_setting(MOBILE_PORT_KEY, &info.port.to_string());
    }
    Ok(info)
}

#[tauri::command]
pub fn mobile_sync(state: State<'_, MobileShared>, rows: Vec<MobileRow>) {
    mobile::sync(&state, rows);
}

#[tauri::command]
pub fn mobile_rows(state: State<'_, MobileShared>) -> Vec<MobileRow> {
    mobile::rows(&state)
}

#[tauri::command]
pub fn mobile_set_check(
    state: State<'_, MobileShared>,
    waybill: String,
    inbound: String,
    name: String,
    checked: bool,
) {
    mobile::set_check(&state, &waybill, &inbound, &name, checked);
}

#[tauri::command]
pub fn mobile_stop(state: State<'_, MobileShared>) {
    mobile::stop(&state);
}

#[tauri::command]
pub fn mobile_info(state: State<'_, MobileShared>) -> Option<MobileInfo> {
    mobile::info(&state)
}

/// 一键放行局域网端口（Windows 防火墙入站）。
///
/// 先直接跑一次（程序本身是管理员时不用弹 UAC）；失败再 Start-Process -Verb RunAs 请用户确认。
/// 最后用 show rule 复核，别只信 exit code。
/// 从「本次要出货」的截图里认出入库号（一张或多张）。
///
/// 复用图片解析那条链（PaddleOCR → 切行切列 → core 解析），只看每行的入库号：
/// 品名容易差一个字/空格，入库号是唯一键，对不上就当场报出来。
#[tauri::command]
pub fn parse_ship_images(paths: Vec<String>) -> Result<serde_json::Value, ApiError> {
    let mut codes: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for path in paths {
        match crate::ocr::parse_image(&path) {
            Ok(sheet) => {
                for item in sheet.items {
                    let code = item.inbound.trim().to_string();
                    if !code.is_empty() && !codes.contains(&code) {
                        codes.push(code);
                    }
                }
            }
            Err(e) => {
                let name = std::path::Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or(path.clone());
                failed.push(format!("{name}：{e}"));
            }
        }
    }
    Ok(serde_json::json!({ "codes": codes, "failed": failed }))
}

/// 产品标签照片 → 编码 + 名称（一张图一条），给「快速解析」的图片导入用。
///
/// OCR 要跑几秒，放 spawn_blocking，别占 Tauri 主线程。
#[tauri::command]
pub async fn parse_label_images(paths: Vec<String>) -> Result<serde_json::Value, ApiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut rows: Vec<serde_json::Value> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for path in paths {
            match crate::ocr::parse_label_image(&path) {
                Ok((code, name)) => {
                    rows.push(serde_json::json!({ "code": code, "name": name }))
                }
                Err(e) => {
                    let label = std::path::Path::new(&path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.clone());
                    failed.push(format!("{label}：{e}"));
                }
            }
        }
        serde_json::json!({ "rows": rows, "failed": failed })
    })
    .await
    .map_err(|e| ApiError::new("ocr", format!("识别任务异常结束：{e}")))
}

#[tauri::command]
pub fn mobile_allow_firewall() -> Result<String, ApiError> {
    let bat = std::env::temp_dir().join("pdfprint-allow-mobile.bat");
    let script = [
        "@echo off",
        "netsh advfirewall firewall delete rule name=\"PdfMobileReceive\" >nul 2>&1",
        "netsh advfirewall firewall delete rule name=\"PdfMobileReceipt\" >nul 2>&1",
        // any：TCP 17650-17652 给网页，UDP 17655 给 App 自动发现
        "netsh advfirewall firewall add rule name=\"PdfMobileReceive\" dir=in action=allow protocol=any localport=17650-17655 >nul",
        "exit /b %errorlevel%",
    ]
    .join("\r\n");
    std::fs::write(&bat, script)
        .map_err(|e| ApiError::new("firewall", format!("写临时脚本失败：{e}")))?;

    // 两条都先落成 String，别在 args 里塞 &format!(..) / to_string_lossy().as_ref()：
    // 前者类型对不上（&String vs &str），后者借的是马上要 drop 的临时值。
    let elevated_cmd = format!(
        "Start-Process -FilePath '{}' -Verb RunAs -Wait -WindowStyle Hidden",
        bat.display()
    );
    let bat_str = bat.to_string_lossy().to_string();

    let run = |elevated: bool| -> bool {
        let status = if elevated {
            std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", elevated_cmd.as_str()])
                .status()
        } else {
            std::process::Command::new("cmd")
                .args(["/c", bat_str.as_str()])
                .status()
        };
        status.map(|s| s.success()).unwrap_or(false)
    };

    let mut ok = run(false);
    if !ok {
        // 没权限：请一次 UAC（用户点否这里会失败，照常返回提示）
        ok = run(true);
    }

    // 复核：规则真的在才算成功
    let verified = std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "show", "rule", "name=PdfMobileReceive"])
        .output()
        .map(|o| {
            let text = String::from_utf8_lossy(&o.stdout).to_string()
                + &String::from_utf8_lossy(&o.stderr);
            text.contains("PdfMobileReceive")
        })
        .unwrap_or(false);

    if verified {
        Ok("已放行 17650-17655（TCP 网页 + UDP 自动发现，专用/公用网络都放行）。手机重新连一下试试".to_string())
    } else if ok {
        Err(ApiError::new("firewall", "命令执行了，但查不到规则，防火墙可能被组策略接管"))
    } else {
        Err(ApiError::new(
            "firewall",
            "没放行成功：可能是 UAC 里点了「否」，或者没有管理员权限",
        ))
    }
}


