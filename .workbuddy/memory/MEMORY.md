# 项目长期备忘

## 构建与发布流程（2026-11-10 用户要求）
- 替换/升级 desktop.exe 时，必须**同时**重新生成安装包：`desktop/src-tauri/target/release/installers/PdfBatchPrint`（NSIS，`npm run tauri build` 或等价命令生成）。
- 单独 `cargo build --release` 只产出裸 exe，不会更新安装包；cargo test 也不会更新 exe。
- 发布交付 zip 时：重新打包 exe + `data/mobile-app/update.json`（页面包）+ 安装包三者都要是新的。
- **不要用 `cargo build --release` 出生产 exe**：不内嵌前端（dist），启动后 WebView 会去连 localhost:1420 显示"拒绝连接"。必须用 `npm run tauri build`（同时产出安装包）。
