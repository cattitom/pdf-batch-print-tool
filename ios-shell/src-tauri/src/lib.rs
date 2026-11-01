//! 出货助手的壳：界面就是 web/ 里那个「填地址 + iframe」的单页，
//! 没有后端逻辑，手机打开后连的是电脑上跑着的局域网服务。
//!
//! iOS 打包交给 CI（.github/workflows/ios-ipa.yml），这里只保留最小入口。

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
