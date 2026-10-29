# 批量 PDF 打印工具

线下打包/发货的批量 PDF 拼版与面单生成工具，两个端共用一套业务逻辑：

| 目录 | 是什么 | 技术栈 |
|------|--------|--------|
| `desktop/` | 主程序：桌面客户端（PDF 拼版、录货清单、箱规计划、箱唛生成、产品标签、设置） | Tauri 2 + React 19 + Rust |
| `boxplan-pwa/` | 「箱规计划」手机版 PWA，给 iPhone 单独用 | 纯原生 JS/CSS，无构建步骤 |

## 快速开始

- 开发、测试、打包桌面端：见 [`desktop/BUILD.md`](desktop/BUILD.md)
- 箱规 PWA：整目录是静态站点，直接打开 `index.html` 或部署到任意静态托管

## OCR 模型（不进仓库）

「录货清单」拖截图时优先用 PaddleOCR 识别，模型是二进制（约 40MB），没进版本库。
需要的话把这三个文件下载到 `desktop/models/`：

| 文件 | 用途 | 大小 | 下载 |
|---|---|---|---|
| `PP-OCRv6_medium_det.mnn` | 文字检测（最高精度档） | 29.6 MB | [下载](https://raw.githubusercontent.com/zibo-chen/ocr-rs/next/models/PP-OCRv6_medium_det.mnn) |
| `PP-OCRv6_small_rec.mnn` | 文字识别 | 10.2 MB | [下载](https://raw.githubusercontent.com/zibo-chen/ocr-rs/next/models/PP-OCRv6_small_rec.mnn) |
| `ppocr_keys_v6_small.txt` | 字典 | 75 KB | [下载](https://raw.githubusercontent.com/zibo-chen/ocr-rs/next/models/ppocr_keys_v6_small.txt) |

或者一条命令搞定：

```powershell
$base = 'https://raw.githubusercontent.com/zibo-chen/ocr-rs/next/models/'
cd desktop\models
foreach ($f in 'PP-OCRv6_medium_det.mnn','PP-OCRv6_small_rec.mnn','ppocr_keys_v6_small.txt') {
    Invoke-WebRequest ($base + $f) -OutFile $f
}
```

- **不装也能用**：没有模型时自动回退 Windows 自带 OCR，只是数字容易读丢几下
- 出自仓库 [`zibo-chen/ocr-rs`](https://github.com/zibo-chen/ocr-rs) 的 `models/` 目录，MNN 格式可直接用
- 为什么检测用 medium、识别用 small：见 [`desktop/models/README.md`](desktop/models/README.md)

## 仓库约定

- 主线分支 `main`。历史连续自早期 Streamlit 版（2026-10-21 下线，逻辑已移植进 `desktop/core/`）
- 构建产物（`target/`、`node_modules/`、`dist/`）与运行时数据（日志、SQLite）不入库
- OCR 模型放 `desktop/models/`，二进制不入库，按上面那节单独下载
- `.workbuddy/memory/` 是开发过程日志（决策与踩坑记录），随项目一起入库

## 分层

```
desktop/core/        核心库，与 UI 无关，可独立 cargo test
desktop/src-tauri/   Tauri 壳 + 命令桥接（JSON 契约 camelCase）
desktop/src/         React 前端（pages/ 一页一个功能）
```

新增业务逻辑先进 `core/`，`commands.rs` 只做参数转换与错误映射。

## 远程仓库

```
https://github.com/cattitom/pdf-batch-print-tool   （private）
```

推送走 GitHub CLI 的 OAuth 凭据，`git push` 不用输密码。换机器要先
`winget install GitHub.cli` → `gh auth login` → `gh auth setup-git`。
