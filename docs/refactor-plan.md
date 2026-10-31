# 重构待办（按优先级）

> 现状：59 个源文件 / 约 1.86 万行。整体分层是清楚的（core ↔ tauri ↔ React），问题只在**少数几个巨型文件**。
> 原则：能编译验证才动手（`cargo check` + `npx tsc --noEmit`），一次只拆一个文件，拆完立刻验证，不做「顺手重命名」。

## 优先级一览

| # | 任务 | 收益 | 风险 | 验证 |
|---|---|---|---|---|
| 1 | `commands.rs` 拆出云同步 | 1234 行 → 1008 行，云同步自成模块 | 低（纯搬移） | cargo check + 手动点一次同步 |
| 2 | `PackingPage.tsx` 拆 hook / 弹窗 | 2206 行 → 1856 行 | 中（状态耦合） | tsc + 页面功能逐项点 |
| 3 | `App.css` 按页面拆分 | 2754 行 → 每页 200～400 行 | 低（纯样式搬家） | tsc + 截图对比 |
| 4 | `parse_packing.rs` 拆解析器/表头/测试 | 1304 行 → 三块 | 中（核心逻辑被多处调用） | cargo test + 云表格对拍 |
| 5 | `xls.rs` 记录改成偏移视图 | open 228ms → 预计 40ms 级，省几十 MB | 中（手写二进制解析） | 对拍 `items/qty` 完全一致 |
| 6 | `ocr.rs` 按引擎拆 | 1161 行 → 3 块 | 低 | cargo check + 截图识别一次 |
| 7 | 绕开 `desktop/` 的 DENY ACL | 不用动系统权限也能出包（构建到 `dist-build`） | 低 | `npx tauri build --no-bundle --config ...` 成功 |

## 1. `commands.rs` 拆出云同步（先做这个）

- **现状**：`desktop/src-tauri/src/commands.rs` 1234 行，云同步整段（`KDOCS_PULL_MJS`、`KdocsSyncDto`、`kdocs_workdir`、`write_script`、`Ran`、`spawn_bounded`、`sync_kdocs_packing`）从 785 行到 1015 行。
- **做法**：新建 `desktop/src-tauri/src/commands/kdocs.rs`（`commands.rs` 旁边建同名目录即可，Rust 允许文件模块带子模块），把整段原样搬进去；`commands.rs` 里只留 `pub mod kdocs;`。
- **连带**：`lib.rs` 的 `generate_handler!` 里那条改成 `commands::kdocs::sync_kdocs_packing`（命令宏的辅助符号在定义模块里，不能只靠 re-export）。
- **验证**：`cargo check --offline`；启动后点一次「从云表格同步」。
- **不要**：顺手把 `parse_packing_xls` 也搬走——它属于「装箱单解析」，不属于云同步。

## 进度

- ✅ 1 `commands.rs` 拆出 `desktop/src-tauri/src/commands/kdocs.rs`（1234 → 1008 行），`cargo check` 通过。
- ✅ 2 `PackingPage.tsx` 2206 → **1856 行**：三个弹窗拆到 `src/pages/packing/`（`StatsModal.tsx` 114 / `MobileSheetModal.tsx` 88 / `LogModal.tsx` 185，连 `parseLog` 一起搬），云同步整段抽成 `useCloudSync.ts`（状态 + 版本号缓存写回 + 10 分钟检查 + 并发/失败去重都在 hook 里，页面只留 `onApplied/onHint/onError` 三个回调）；`tsc` 通过，页面截图与基线 **sha256 一致**，点「从云表格同步」提示「已从云表格同步「11月」：78 条待办」。
- ✅ 3 `App.css` 2754 行拆成 `src/styles/` 十个分块，`App.css` 只剩 `@import`（顺序 = 拆分前顺序）；截图与基线 sha256 一致，dev server 确认 `@import` 已内联。
- ✅ 4 `parse_packing.rs` 1304 行拆成 `parse_packing/{mod,text,table,xlsx,tests}.rs`；`cargo check` 零 warning，`cargo test` 34 passed，`dump_packing` 对拍 `sheet=11月 items=78 qty=453` 一致。
- ✅ 7 不动 ACL 出包：`npx vite build --outDir dist-build` + `npx tauri build --no-bundle --config '{"build":{"beforeBuildCommand":"","frontendDist":"../dist-build"}}'` → `desktop/src-tauri/target/release/desktop.exe`（20MB，`data/kdocs` 缓存落在 exe 旁）。
- ⏸ 5 / 6 暂缓：`xls.rs` 单表 2.7ms 无感知；`ocr.rs` 拆分只影响可读性且需真图片回归。

## 2. `PackingPage.tsx` 拆 hook 与弹窗

- **可拆的三块**（按耦合从松到紧）：
  1. 云同步整段（`syncFromCloud`、两个 `useEffect`、`cloudSheet/cloudSheets/cloudUpdate/autoSync`）→ `desktop/src/pages/packing/useCloudSync.tsx`；
  2. 统计弹窗 → `StatsModal.tsx`（props：`customerStats/statBoxes/filtered/shownOrder/onCopy/onClose`）；
  3. 历史/日志弹窗 → `LogModal.tsx`。
- **风险点**：`syncFromCloud` 依赖 `applySheet`、`savedPath`、`rowsRef`、`syncBusyRef`，抽 hook 要么把这些也传进去，要么把 `applySheet` 一起挪。建议先抽**统计弹窗**（纯展示）练手，再抽云同步。
- **验证**：`npx tsc --noEmit`；逐项点：同步、换表、统计、复制、看板切换。

## 3. `App.css` 按页面拆分

- 拆成 `styles/base.css`（变量/reset/按钮）、`styles/packing.css`、`styles/impose.css`、`styles/boxplan.css`、`styles/label.css`、`styles/mobile.css`，在 `App.css` 里 `@import` 合并。
- 注意：`@import` 必须放在 CSS 最前面；Vite 会内联，构建产物不变。

## 4. `parse_packing.rs` 拆三块

- `parse_packing/table.rs`（合并单元格、读表头、`parse_table`）、`parse_packing/parse.rs`（`parse_file/parse_text/parse_xls_file/parse_grid`）、`parse_packing/tests.rs`（现有 `#[cfg(test)]`）。
- 对外仍 `pub use` 原有符号，`lib.rs` 不动。

## 5. `xls.rs` 性能（可选，等真有性能痛点再做）

- `collect_records` 现在给每条记录复制一份 `Vec<u8>`（这份 4.4MB 的文件 110 万+ 条记录，open 228ms、峰值内存几十 MB）。
- 改成「记录表只存 `(rid, offset, len)`，body 用切片按需取」，open 预计降到 40ms 级。
- 对拍方式：同一份文件 `items` 数、`qty_sum` 必须与改前完全一致（现在 11月 = 78 条 / 453 件）。

## 7. release 构建被 ACL 卡住（影响出包，优先级实际上不低）

- `desktop/` 带 `Everyone:(I)(CI)(DENY)(DC)` 继承，`vite build` 清 `dist/assets` 报 `EPERM`，所以 `desktop/dist` 一直是旧产物。
- 修法（需管理员）：`icacls desktop /remove:d Everyone`；或只对 `desktop/dist` 去掉继承：`icacls desktop\dist /inheritance:e /grant "%USERNAME%":(OI)(CI)F`。
- 不动权限的话：dev 模式不受影响，但出不了新 release 包。

## 明确不做

- 不引入状态管理库（Redux/Zustand）：当前页面间几乎不共享状态，加了反而多一层。
- 不把 `mobile/` 的 HTML/CSS/JS 改写成 React：它要 `<100KB`、离线、编进 Rust 二进制，现在的原生三件套最稳。
- 不动 `boxplan-pwa/`：那是独立小应用，跟主程序没有代码共享。
