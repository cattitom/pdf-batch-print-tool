# 出货助手（iOS 壳）

手机打开即进入本地工作台，安装包内置可离线使用的基础界面；已配对电脑可提供整包页面热更新。
清单页只向电脑请求 `/api/sheet`、`/api/check` 和 `/api/claim` JSON 数据，不再加载电脑提供的 HTML。

- 入库与出货：统一实时与本地清单入口，先显示手机快照，再检查电脑连接；离线仍可查看和记录，无快照时显示配对引导。
- 工作台、清单与设置切换带轻量过渡动画，遵循系统“减少动态效果”偏好。
- 设备与连接：管理扫码配对及已保存电脑，断网不会阻塞工作台。

每次读取或在线出货成功都会更新手机快照。离线出货操作保存为待同步队列，保留首次操作版本；
重连后按顺序补传。版本冲突保留并明确提示，核对最新清单后重新操作。不同电脑的快照和待同步操作不会混用。
首次获取数据、领取任务和实时同步仍需要电脑在线。手机数据保存在 App 本地存储中，卸载 App 会清除。

## 本地页面资源

`node ios-shell/scripts/sync-mobile.mjs` 将共享清单界面复制到 `ios-shell/web/shipping/`，
改写成相对资源路径，并加载手机专用 `transport.js`。iOS 构建和本地开发会自动执行，CI 也会显式检查此步骤。
修改共享样式或界面后生成页面更新包即可；只有固定启动器、更新协议或原生功能变化才需要重新打包 IPA。电脑端的清单页面作为浏览器访问兼容入口保留。

## 局域网页面热更新（自用）

首次使用需要更新一次电脑程序，并覆盖安装带更新启动器的 IPA。请沿用原来的 App 标识和签名，不要卸载旧 App，以保留配对与离线数据。

- `web/index.html` 和 `web/update-loader.js` 是固定启动器；`web/app.html` 是可更新首页。
- 页面包包含首页、清单 HTML、CSS、JS 和数据层 `transport.js`，协议版本 `shellApi: 1`。
- 手机启动、打开已配对电脑的清单时会后台检查更新；也可到「设备与连接 → 检查页面更新」。旧电脑不支持更新时不影响清单。
- 完整下载、SHA-256 校验和协议检查通过后才暂存；不会打断正在处理的清单。点击「应用更新」或下次完整启动时切换。
- 新首页或清单启动失败会自动退回上一版；「恢复内置版」只重置页面版本，不清除配对、清单和待同步操作。
- 替换基础安装包时优先采用新安装包内置页面，避免旧页面缓存覆盖新程序。

仓库开发环境发布页面更新：

```powershell
node ios-shell/scripts/publish-update.mjs
```

生成 `data/mobile-app/update.json`，调试版电脑程序会自动读取。正式电脑程序从 **exe 同目录的 `data/mobile-app/update.json`** 读取：

```powershell
node ios-shell/scripts/publish-update.mjs --output "D:/出货工具/data/mobile-app/update.json"
```

只更新这一个文件即可，无须重新编译或重启电脑程序。发布脚本先写临时文件再替换，电脑的版本接口与整包接口固定版本，避免下载过程中混用两个发布。
未放置更新文件时，电脑提供编译时内置的页面包。已配对手机通过 `/api/app-update/manifest` 和 `/api/app-update/bundle` 下载；两个接口均校验配对口令。
更新包摘要检查用于发现损坏或混版；局域网 HTTP 不提供对抗网络篡改的签名认证。

页面更新保存在 App 同源本地存储中，不改变清单数据格式。新增原生命令、更新启动器，或者变更电脑端的 Rust 校验和数据 API，仍需更新相应程序。

## 自动发现和扫码配对

1. Bonjour TXT 只包含电脑名；UDP 应答只包含名称、端口和无凭证的地址。
2. 首次连接必须扫描电脑「手机出货」面板中的二维码，访问凭证保存在手机本地。
3. 已配对后，发现服务可找到电脑；仅在地址的协议、主机及端口与已保存链接一致时，才使用本机保存的凭证重连。
4. 未配对的电脑会提示扫码。电脑更换口令或旧版本升级后，也需要重新扫码。

发现服务不再分发访问口令。历史链接只存手机本地，换端口后需要重新扫码，避免把凭证发给其他服务。

前提：

- 手机和电脑在同一局域网（同一 WiFi / 电脑热点）；
- 电脑防火墙放行（面板里的「一键放行」会把 **TCP 17650-17655 + UDP** 一起放行）；
- iOS 首次会弹「本地网络」和「相机」权限，都要允许（`Info.plist` 的 `NSLocalNetworkUsageDescription`
  / `NSCameraUsageDescription` / `NSBonjourServices` 由 CI 注入）。

## 出 IPA（不需要 Mac）

1. 推到 GitHub（仓库：`cattitom/pdf-batch-print-tool`）；
2. Actions → **iOS 出货助手 IPA** → Run workflow；
3. 跑完在 Summary 下方下载 `shipping-shell-ipa`，里面是 `shipping-shell-unsigned.ipa`。

CI 做的事：`tauri ios init` 生成 Xcode 工程 → 关签名 → 注入 ATS 和本地网络权限 →
`tauri ios build --debug` 编译 → 把 DerivedData 里的 `.app` 塞进 `Payload/` 打成 `.ipa`。
**产物未签名**，用你自己的证书签（AltStore / 爱思 / `codesign` 都行）。

## 本地调试桌面版

```bash
cd ios-shell/src-tauri
cargo run            # 会弹出 390×844 的窗口，跟手机上长得一样
```

注意：工作台和已缓存清单可独立打开。获取实时数据时，需要电脑端手机服务在线并完成配对。

## 已知限制

- 只能在**同一子网**发现（跨网段/路由器隔离不行）；
- 局域网 http 明文靠 ATS 放行，正式分发前可以换成自签 https（可选）。
