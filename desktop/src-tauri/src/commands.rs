//! Tauri commands：前端 ↔ pdfimpose-core 的桥接层。
//!
//! 设计要点：
//! - 文件一律传**路径**，由 Rust 端读取字节，避免大文件走 IPC
//! - 拼版输出直接写盘（前端先用 dialog 选好保存路径），只回传路径与统计
//! - 进度通过事件 `impose-progress` 推送（done, total）
//! - Db 用 Mutex 包住（rusqlite Connection 非 Sync）

use base64::Engine as _;
use pdfimpose_core as core;
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

// ---------- 设置 ----------

#[tauri::command]
pub fn all_settings(state: State<AppState>) -> Result<std::collections::HashMap<String, String>, ApiError> {
    Ok(db(&state).all_settings().map_err(ApiError::from)?.into_iter().collect())
}

#[tauri::command]
pub fn set_setting(state: State<AppState>, key: String, value: String) -> Result<(), ApiError> {
    db(&state).set_setting(&key, &value).map_err(ApiError::from)
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
}
