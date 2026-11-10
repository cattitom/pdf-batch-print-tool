pub mod commands;
mod config_file;
mod logging;
mod mobile;
mod mobile_master;
mod mobile_update;
mod wechat;
// 示例 examples/label_ocr.rs 直接调 OCR 跑真图验证，所以这里 pub
#[cfg(windows)]
pub mod ocr;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // 日志先于一切：后面任何失败都要留下线索
            let dir = app.path().app_data_dir()?;
            let log_file = logging::init(&dir);
            logging::install_panic_hook();
            log::info!(
                "启动 PdfBatchPrint v{} | 日志 {}",
                env!("CARGO_PKG_VERSION"),
                log_file.display()
            );
            let state = commands::init_state(app)?;
            let mobile_state = mobile::with_db(state.db.clone());
            app.manage(state);
            app.manage(mobile_state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::pdf_infos,
            commands::preview_pdf,
            commands::impose_pdfs,
            commands::open_pdf_in_wps,
            commands::add_print_history,
            commands::list_print_history,
            commands::delete_print_history,
            commands::clear_print_history,
            commands::generate_labels,
            commands::preview_label,
            commands::list_label_docs,
            commands::latest_label_doc,
            commands::generate_product_labels,
            commands::preview_product_label,
            commands::all_settings,
            commands::config_file_path,
            commands::set_setting,
            commands::ledger_mode,
            commands::ledger_archive_tickets,
            commands::ledger_archive_save,
            commands::set_packing_check,
            commands::set_packing_checks,
            commands::list_packing_checks,
            commands::clear_packing_checks,
            commands::log_dir,
            commands::cache_packing_cargo,
            commands::packing_cargo,
            commands::parse_packing_text,
            commands::parse_packing_file,
            commands::parse_packing_image,
            commands::parse_packing_xls,
            commands::kdocs::sync_kdocs_packing,
            commands::kdocs::snapshot_kdocs_cache,
            commands::kdocs::open_kdocs_login,
            commands::save_text_file,
            commands::read_log_tail,
            commands::clear_log,
            commands::mobile_start,
            commands::mobile_reset_token,
            commands::mobile_sync,
            commands::mobile_set_check,
            commands::mobile_rows,
            commands::save_mobile_master_edit,
            commands::take_mobile_cloud_refresh,
            commands::mobile_last_activity,
            commands::finish_mobile_cloud_refresh,
            commands::mobile_stop,
            commands::mobile_info,
            commands::mobile_allow_firewall,
            commands::parse_ship_images,
            commands::parse_label_images,
            commands::wechat::wechat_status,
            commands::wechat::wechat_check_dir,
            commands::wechat::wechat_scan,
            commands::wechat::wechat_rooms,
            commands::wechat::wechat_messages,
            commands::wechat::wechat_poll,
            commands::wechat::wechat_export_csv,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}



