# 批量 PDF 打印工具

线下打包/发货的批量 PDF 拼版与面单生成工具，两个端共用一套业务逻辑：

| 目录 | 是什么 | 技术栈 |
|------|--------|--------|
| `desktop/` | 主程序：桌面客户端（PDF 拼版、录货清单、箱规计划、箱唛生成、产品标签、历史、设置） | Tauri 2 + React 19 + Rust |
| `boxplan-pwa/` | 「箱规计划」手机版 PWA，给 iPhone 单独用 | 纯原生 JS/CSS，无构建步骤 |

## 快速开始

- 开发、测试、打包桌面端：见 [`desktop/BUILD.md`](desktop/BUILD.md)
- 箱规 PWA：整目录是静态站点，直接打开 `index.html` 或部署到任意静态托管

## 仓库约定

- 主线分支 `main`。历史连续自早期 Streamlit 版（2026-10-21 下线，逻辑已移植进 `desktop/core/`）
- 构建产物（`target/`、`node_modules/`、`dist/`）与运行时数据（日志、SQLite）不入库
- OCR 模型放 `desktop/models/`，二进制不入库，按该目录的 `README.md` 单独下载
- `.workbuddy/memory/` 是开发过程日志（决策与踩坑记录），随项目一起入库

## 分层

```
desktop/core/        核心库，与 UI 无关，可独立 cargo test
desktop/src-tauri/   Tauri 壳 + 命令桥接（JSON 契约 camelCase）
desktop/src/         React 前端（pages/ 一页一个功能）
```

新增业务逻辑先进 `core/`，`commands.rs` 只做参数转换与错误映射。
