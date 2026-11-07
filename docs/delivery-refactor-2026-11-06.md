# 交付配置与出货页面拆分说明（2026-11-06）

> 后续调整：精简版及系统 OCR 已移除，以下双版本内容为历史记录，当前以《仅保留完整版与 PaddleOCR》说明为准。

## 范围调整

按用户要求，不修改 boxplan-pwa 箱规计划工具。阶段 6 处理 Node 运行条件、OCR 模型及打包检查、出货页面职责拆分。

## Node 运行条件

- 最低 Node.js 22.12.0，已验证 24.12.0，提供 .node-version 和 .nvmrc。
- 桌面启动、前端构建及独立 WPS 取数使用同一运行条件检查，验证版本、fetch 和 WebSocket。
- 运行检查模块与 WPS 脚本一起嵌入 Rust 二进制并在运行期写出，避免分发时遗漏依赖。

## 模型与打包

- pnpm models:download 下载模型并校验文件大小及 SHA-256；已校验文件不重复下载。
- 模型清单记录当前已验证模型的哈希；下载结果不匹配时停止，不接受未知变更。
- 基础 Tauri 配置不要求模型；完整版通过独立配置添加模型资源。
- pnpm release 构建完整版，pnpm release:lite 构建不携带模型的精简版；无可用模型时沿用 Windows OCR 回退。
- --check 仅检查运行条件、Rust 及完整版模型。缺失模型时给出中文下载和精简版指引。
- 使用 dist-build 输出前端，避开旧 dist 目录的删除权限限制，不更改系统 ACL。
- 安装包副本及便携包按类型、时间保留，不先递归删除已有便携目录。

## 出货页面拆分

- usePackingChecks.ts：事务保存、失败重试、版本与来源检查、手机快照读取。
- recordKey.ts：统一记录键，页面和保存模块共用。
- PackingSaveStatus.tsx：保存中、保存失败、重试和放弃提示。
- 页面保留表格操作与数据导入，避免一次拆分过多相互依赖的功能。

## 验证结果

- 前端与构建前检查回归：13 项通过。
- Rust 核心库：58 项通过、1 项跳过。
- 桌面 Rust 全量测试：47 项通过、1 项跳过（含 OCR 样例）。
- 总计 118 项通过、2 项跳过。
- TypeScript 检查通过，完整版和精简版 NSIS、便携 ZIP 均构建成功。
- 完整版 ZIP 的模型哈希校验通过；精简版 ZIP 和 NSIS 均确认不含模型。
- 完整版 NSIS 约 27.69 MiB，精简版约 7.61 MiB。
- boxplan-pwa 无差异。

## 边界

未进行物理打印、真实 WPS 云表或 iPhone 联调，未重新构建 iOS IPA。
精简版仅是不携带模型；若机器上另有可用模型，仍遵循既有 OCR 模型搜索规则。
箱规 PWA 保持不变。所有修改保留在当前工作区，未提交或推送。

## 本次产物

- full 安装包：`desktop\src-tauri\target\release\installers\PdfBatchPrint-full-2026-11-06T15-37-24-877Z\PdfBatchPrint_0.1.0_x64-setup.exe`
- full 便携 ZIP：`desktop\src-tauri\target\release\PdfBatchPrint-full-2026-11-06T15-37-24-877Z.zip`
- lite 安装包：`desktop\src-tauri\target\release\installers\PdfBatchPrint-lite-2026-11-06T15-40-17-709Z\PdfBatchPrint_0.1.0_x64-setup.exe`
- lite 便携 ZIP：`desktop\src-tauri\target\release\PdfBatchPrint-lite-2026-11-06T15-40-17-709Z.zip`
