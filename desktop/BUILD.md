# 批量 PDF 打印工具（桌面版）

基于 **Tauri 2 + React 19 + Rust** 的批量 PDF 打印拼版客户端。早期 Streamlit 版已下线，
其逻辑全部移植到本项目（`core/`），性能大幅提升（13 万页拼版 ~1s，pypdf 版约 9s）。

## 功能

| 页签 | 功能 |
|------|------|
| 🖨 PDF 拼版 | 多 PDF 合并、按目标页数凑页/截取、逐份旋转、自动去白边/手动裁剪、纸张缩放（方向自动跟随）、文件间分隔页、进度条、输出名自动生成 |
| 🏷 箱唛生成 | 一箱一页，Code128 条码（入库单号）+ 二维码（单号-S货架-箱号）+ 中文排版；固定 100×100mm 热敏标签；首页预览；自动带出上次单据 |
| 🕘 历史 | 打印/生成历史（SQLite 本机存储），可删除/清空 |
| ⚙️ 设置 | 本机偏好（key-value） |

## 目录结构

```
desktop/
├── src/                # React 前端
│   ├── api.ts          #   后端命令的类型化封装（JSON 契约见 src-tauri/src/commands.rs）
│   └── pages/          #   ImposePage / LabelPage / HistoryPage / SettingsPage
├── src-tauri/          # Tauri 壳 + 命令桥接层
│   └── src/commands.rs #   13 个 command；文件传路径不走 IPC；进度走 impose-progress 事件
├── core/               # pdfimpose-core 核心库（与 UI 无关，可独立测试）
│   └── src/
│       ├── impose.rs   #   拼版合并（lopdf；旋转烘焙进 cm 矩阵；纸张方向跟随）
│       ├── crop.rs     #   内容包围盒检测（pdfium-render，自动去白边）
│       ├── labels.rs   #   箱唛生成（genpdf + barcoders + qrcode）
│       ├── flatten.rs  #   扁平化：箱唛整份转 300dpi 灰度图，避免 CJK 字体嵌入膨胀
│       └── db.rs       #   SQLite（rusqlite bundled；历史/单据/设置）
└── e2e/
    └── e2e-tauri.cjs   # 端到端测试（CDP 连 WebView2）
```

## 开发

前置：Node 22+、pnpm、Rust（MSVC）、WebView2。

双击 `dev-start.bat`（等价于下面的命令）。脚本会检查 pnpm/Rust、提示 1420 端口是否已被占用、
把日志级别设为 `debug`，并在启动前告诉你是否还有 desktop.exe 占着 `target\debug`。

```bash
cd desktop
pnpm install
pnpm tauri dev        # 开发窗口（vite :1420 热更新）
```

开发模式的日志同样写在 `%APPDATA%\com.hanlin.pdfprint\logs\app.log`。

## 测试

```bash
# 核心库单元测试（23 个：拼版计数/缩放旋转裁剪/13万页性能/分隔页缓存/页码线性/
# 坏 PDF 不 panic/纸张缩放方向/bbox 对齐 pymupdf/箱唛/SQLite）
cd core && cargo test --release

# 桥接层 JSON 契约测试（含坏输入的字段名锁定）
cd src-tauri && cargo test

# 端到端（先启动 dev 窗口 + CDP 调试端口）
pnpm tauri dev &
# 用 CDP 启动 exe（见 e2e 脚本头注释），然后：
NODE_PATH=<playwright-core 所在目录> node e2e/e2e-tauri.cjs
```

E2E 覆盖：UI 页签渲染、pdf_infos、真实拼版（2 文件 + 自动裁剪 + 旋转 90° + A4 纸 + 分隔页 = 7 页）、
历史读写、箱唛预览 base64。输出另经 pymupdf 逐页交叉验证（页数/尺寸/旋转/文本）。

测试 PDF 固定放在 `core/tests/fixtures/`（8 个 1–2KB 的小文件），用 `CARGO_MANIFEST_DIR`
定位，不再依赖项目外目录。

## 前端类型（自动生成）

`src/bindings/*.ts` 由 ts-rs 从 Rust 生成，**不要手改**。改了 DTO 之后：

```bash
powershell -File scripts\gen-bindings.ps1
```

`src/api.ts` 只做一层别名（`CropSpec = CropDto` 等）与命令封装，类型本身全部来自生成文件。
`cargo test` 里的 `export_bindings` 测试也会顺带刷新这些文件。

## 打包

双击 `build-release.bat`（或在本目录执行）即可，脚本会检查 pnpm/Rust 环境、确认程序没在运行、
执行 `pnpm tauri build`，最后列出产物与大小；额外参数会透传（如 `build-release.bat --bundles nsis`）。

```bash
# 等价的手动命令
pnpm tauri build
# 产物（tauri.conf.json 里 targets = ["nsis"]）：
#   src-tauri/target/release/bundle/nsis/*.exe   (NSIS 安装程序)
#   src-tauri/target/release/desktop.exe         (绿色单文件，可直接运行)
```

**不出 MSI**：MSI 走 WiX 工具链、体积更大、且是 per-machine（要管理员），本项目不面向域环境批量分发，
所以默认只出 NSIS + 绿色 exe。偶尔需要 MSI 时临时覆盖即可，不必改配置：

```bash
pnpm tauri build --bundles msi
```

## 日志

release 版没有控制台，日志落在：

```
%APPDATA%\com.hanlin.pdfprint\logs\app.log
```

- 超过 2MB 自动轮转一份 `app.log.1`；设置页有「📂 打开日志目录」按钮
- 级别用环境变量 `PDFPRINT_LOG` 控制：`trace` / `debug` / `info`（默认）/ `warn` / `error` / `off`
- 记录内容：启动版本与路径、拼版参数与耗时、箱唛生成、所有错误（带 code）、panic 现场
- 报障时让用户附上这个文件即可

## 性能参考

| 场景 | pypdf（Streamlit 版） | Rust 核心 | 2026-10-21 优化后 |
|------|----------------------|-----------|------------------|
| 13 万页拼版（含写出） | ~9s | ~1.0s | ~1.0s |
| 单文件凑 4 万页 | - | 0.30s | 0.30s |
| 60 文件 × 4 页 + 每文件 1 张分隔页 | - | 78.6s | **0.77s** |
| 5,000 页盖页码 | - | 505s（且页数翻倍耗时 ×7） | **0.06s** |
| 箱唛 300 箱（含 300dpi 扁平化） | - | 8.7s | 9–11s（受机器负载波动） |

优化点：

- 分隔页按 `(尺寸, 文字)` 缓存，字体字节全局只读一次（`font.rs` / `sep.rs`）
- 页码改用 lopdf 直接给每页追加内容流，不再走 pdfium 全文档往返（`numbers.rs`）
- 拼版输出流式写盘（`impose_to`），源文档逐文件解析、用完即释放

## 实现要点（坑）

- pdfium-render 0.9.4：可变访问用 `pages_mut()`/`fonts_mut()`；CJK 字体必须 `is_cid=true` 嵌入，否则字形乱码
- 旋转烘焙进内容 `cm` 矩阵而非 `/Rotate` 标记——与纸张缩放叠加时数学才正确
- Code128（barcoders 2.0）需要显式 START-B 前缀 `Ɓ`
- genpdf：字号 `u8`；`Mm` 最大 `u16`；图片必须 RGB 无 alpha
- 前后端 JSON 契约：camelCase；`CropSpec` 为 `{type:"auto"|"rect"|"manual", ...}` tagged enum
