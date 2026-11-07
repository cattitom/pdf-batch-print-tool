# 仅保留完整版与 PaddleOCR（2026-11-06）

## 修改内容

- 删除 Windows 系统 OCR 的识别实现、COM/WinRT 异步适配和全部回退调用。
- 清理仅为系统 OCR 使用的直接依赖及 Windows API 特性；其他功能需要的 Windows API 保留。
- 截图、装箱单、标签图片只使用 PaddleOCR；缺少模型、初始化失败或识别无结果时明确报错。
- 移除精简版 npm 入口，旧 --lite 参数明确拒绝；默认 Tauri 配置始终包含三个必需模型，直接构建也先检查模型。
- 完整版安装程序、便携包及模型 SHA-256 检查继续保留。
- 界面、当前构建文档和模型说明不再提供系统 OCR 或精简版选项。
- 先前生成的精简版 ZIP、安装包和解压目录移到 .tmp/retired-lite-2026-11-06，避免与当前发行包混用。

## 验证

前端及打包回归测试 14 项通过，TypeScript 检查通过。
桌面全量测试 48 项通过、1 项跳过；最终 OCR 针对性测试 6 项通过，包含三张真实截图及缺模型错误路径。
新完整版 NSIS 安装包与便携 ZIP 均已生成，三个模型的资源及 SHA-256 检查通过。

安装包：`desktop\src-tauri\target\release\installers\PdfBatchPrint-full-2026-11-06T15-48-49-272Z\PdfBatchPrint_0.1.0_x64-setup.exe`

便携包：`desktop\src-tauri\target\release\PdfBatchPrint-full-2026-11-06T15-48-49-272Z.zip`

## 边界

未实测物理打印及真实手机联调。模型缺失只影响图片识别，PDF 和表格导入功能仍可使用。
箱规 PWA 未修改。此前双版本说明保留为历史记录，以本说明及当前 BUILD.md 为准。
