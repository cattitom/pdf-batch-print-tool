# 批量 PDF 打印工具

线下打包/发货的批量 PDF 拼版与面单生成工具，两个端共用一套业务逻辑：

| 目录 | 是什么 | 技术栈 |
|------|--------|--------|
| `desktop/` | 主程序：桌面客户端（PDF 拼版、出货清单、箱规计划、箱唛生成、产品标签、设置） | Tauri 2 + React 19 + Rust |
| `boxplan-pwa/` | 「箱规计划」手机版 PWA，给 iPhone 单独用 | 纯原生 JS/CSS，无构建步骤 |

## 快速开始

- 开发、测试、打包桌面端：见 [`desktop/BUILD.md`](desktop/BUILD.md)
- 箱规 PWA：整目录是静态站点，直接打开 `index.html` 或部署到任意静态托管

## 从 WPS 云表格同步（出货清单）

「出货清单」页右上角点「从云表格同步」，直接从 WPS 云文档 [来货登记（新）](https://www.kdocs.cn/l/cie4FmnjnBhW) 拉最新数据；不用再手动导出 xlsx。会自动挑最新一张来货登记表（按表名从后往前，找带「收货仓 / 来货日期 / 入库号」表头且有数据的表），旁边的下拉可以换别的表。

前置条件（缺一个都会在界面报中文错）：

1. **登录态**：先点「出货清单」页头的 **「WPS 登录」**（脚本随程序发，装完在 `<程序目录>\data\kdocs\tools\kdocs-login.bat`；仓库根目录也有同一份），用专门的浏览器配置打开云表格并扫码登录。窗口开着、关掉都行——关掉之后同步会自己起无界面 Chrome。这个配置记着登录状态，下次只需确认一次
2. **Node.js 22.12.0+**（脚本启动时检查版本及 `fetch` / `WebSocket`，本机验证版本 24.12.0）

链路：`kdocs-login.bat` 起的 Chrome（9222 调试端口）→ `tools/kdocs/kdocs-pull.mjs` 在页面里拿 presigned 下载地址取 `.xls` → Rust 直接解 BIFF8（`desktop/core/src/xls.rs`，零依赖）→ 现有解析器变待办。**不再需要 Python**。取数脚本和登录脚本都 `include_str!` 编进 Rust 二进制，运行期写到缓存目录下的 `tools\`，打包不用额外配资源。

**缓存就放在程序旁边**：`<desktop.exe 所在目录>\data\kdocs\`，里面有 `kdocs-latest.xls` / 取数脚本 / 版本号记录。整包拷到另一台电脑（公司机器）时缓存跟着走，进门就能直接铺表。只有 exe 目录写不进去（放到 `C:\Program Files` 之类）才退回 `%LOCALAPPDATA%\pdf-batch-print-tool\kdocs\`。

**打开程序就有表，不用先点**：每次同步成功都会记住下载下来的 `kdocs-latest.xls`（以及表名、云端版本号 `fver`），下次进「出货清单」先把它按上次那张表解析出来铺上，几十毫秒；然后再在后台问一句云端版本号（约 1KB / 0.4 秒，版本没变就**整本都不下载**）：
- 云端没变 → 什么都不做；
- 云端变了 → 顶上亮一条「云表格有新数据（版本 xxx），要换最新吗？」，你点「换最新」才真的换表，不会悄悄把你正在勾的表冲掉；
- 勾了工具栏的「有更新自动换表」才会在发现新版时直接换。

后台检查每 10 分钟一次，跟那个勾选框无关；缓存文件被清理掉了（或第一次用）会自动拉一次铺上。想手动拉，点「从云表格同步」。

> 为什么不用「WPS 云盘同步到本地 + 监控文件」：在线表格在云盘目录里只有一个几百字节的 `.wpsonline` 指针（内容只有 fileid），真实数据一直在云上，WPS 客户端不会把它同步成 xlsx。手动另存出来的是快照，云表格里新加的行不会自己出现。

**不用一直开着浏览器**：同步时如果连不上 9222（也就是没开 `kdocs-login.bat` 那个窗口），脚本会自己起一个无界面 Chrome（`--headless=new`，复用同一个登录配置目录 `%LOCALAPPDATA%\kdocs-cdp-profile`），拉完数据立刻退出。也就是说日常只点一下同步、什么都不用开；只有登录态过期时才需要重新点「WPS 登录」（或双击 `kdocs-login.bat`）扫码。注意同一个配置目录不能同时被两个 Chrome 打开——窗口还开着的话脚本就直接用它，别两边并开（想强制走无界面加 `--no-headless` 之外的行为请先关窗口）。

## 手机出货（局域网）

「出货清单」页右上角点「手机出货」，弹窗里出二维码；手机和电脑连同一个 Wi-Fi，扫码就能勾选。

- 首次连接需扫描电脑面板中的二维码配对；自动发现只公布电脑名称和地址，不提供访问凭证。旧版本升级后需重新扫码
- 手机勾选写入本机数据库成功后才返回成功，桌面轮询只读取结果；记录冲突会要求刷新核对
- 服务只在桌面端运行时存在，表格不落手机；勾选实时回同步，并按原来的方式写进本机记录
- 首次启动 Windows 会弹防火墙授权，要勾「专用网络」并允许；没弹或点掉了，用管理员 PowerShell 跑一次
  `netsh advfirewall firewall add rule name="PdfMobileReceipt" dir=in action=allow protocol=TCP localport=17650-17652`，
  或把当前 Wi-Fi 改成「专用网络」
- 端口默认 `17650`（占用时自动换），口令存在本机、重启不变：手机把地址存成书签就不用每次扫码；换新口令后旧链接失效。关掉弹窗服务即停
- 没「入库号」也没「单号」的行不能出货：桌面端按钮灰着点不动，手机端同样点不动（只弹提示）；这些行照常算进未出、照常出现在手机页和导出里

## OCR 模型（不进仓库）

推荐在 `desktop/` 运行 `pnpm models:download` 下载并校验模型；`pnpm release` 生成包含模型的完整版。

「出货清单」拖截图时优先用 PaddleOCR 识别，模型是二进制（约 40MB），没进版本库。
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

- **OCR 模型必需**：只使用 PaddleOCR，缺少模型会提示补齐模型或重新安装完整版；表格和 PDF 功能不依赖 OCR
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


