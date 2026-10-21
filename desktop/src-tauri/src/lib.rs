mod commands;
mod logging;

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
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::pdf_infos,
            commands::preview_pdf,
            commands::impose_pdfs,
            commands::add_print_history,
            commands::list_print_history,
            commands::delete_print_history,
            commands::clear_print_history,
            commands::generate_labels,
            commands::preview_label,
            commands::list_label_docs,
            commands::latest_label_doc,
            commands::all_settings,
            commands::set_setting,
            commands::log_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
