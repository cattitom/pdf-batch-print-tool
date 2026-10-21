# 项目长期约定（批量 PDF 打印工具）

## 结构
- 单端项目：只维护 `desktop/`（Tauri 2 + React 19 + Rust）。Streamlit Web 版已于 2026-10-21 下线，
  根目录不再放 Python 代码；旧版备份在项目外 `D:\Users\Desktop\test\pdf_batch_print_tool_streamlit_backup_20261021.zip`。
- 分层：`core/`（与 UI 无关的核心库，可独立 `cargo test`）→ `src-tauri/src/commands.rs`（命令桥接，JSON 契约
  camelCase）→ `src/`（React 页面）。
- 打印走 Windows 系统打印，不再依赖 SumatraPDF。

## 业务约定
- 箱唛规格**只有 100×100mm 热敏标签**一种（A4 横向已删除，勿再引入尺寸选项）。
  版式常量在 `core/src/labels.rs` 的 `LABEL_MM` / `LAYOUT`。
- 箱唛一箱一页：Code128（入库单号）+ 二维码（单号-S货架-箱号）+ 中文排版；生成后扁平化为 300dpi
  灰度图，避免 CJK 字体整份嵌入导致体积膨胀。
- 历史库为 SQLite 本机存储（`core/src/db.rs`），改表结构时注意 `CREATE TABLE IF NOT EXISTS` 不会迁移旧库，
  新增列要写 `ALTER TABLE`，删列需确认旧库仍有默认值。

## 验证惯例
- 改核心逻辑：`cd core && cargo test --release`
- 改桥接层：`cd src-tauri && cargo check`
- 改前端：`pnpm exec tsc --noEmit`
- 出包：`pnpm tauri build`（产物在 `src-tauri/target/release/`）
