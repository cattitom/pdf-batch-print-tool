# 出货助手（iOS 壳）

电脑上的出货清单跑在局域网里（`http://<电脑IP>:17650/?t=<口令>`）。这个目录是个**极小的 Tauri 应用**，
只做一件事：**打开就自动找到电脑、全屏显示那个页面**。没有后端、没有 OCR，所以能在 iOS 上编。

## 为什么不做成完整 App

桌面那份程序依赖 Windows 专属的东西（MNN 预编译的 OCR、WPS 注册表、系统打印），iOS 上编不过；
而且手机真正需要的只是「看票 / 领取 / 打勾」。所以这里做的是**瘦客户端**：数据、逻辑全在电脑端。

## 自动发现怎么工作

不用扫码、不用填地址：

1. 电脑端（`desktop/src-tauri/src/mobile.rs`）在开手机服务时，顺手起一个 **UDP 监听 17655**；
2. App 启动时往 `255.255.255.255:17655` 广播暗号 `PDF-MOBILE-DISCOVER`，最多试 6 轮（约 3 秒）；
3. 电脑回 `{port, token}`，App 用回包的源 IP 拼出地址，直接加载；
4. 找不到就显示「没找到电脑，检查是否同一 WiFi」，点重试再来一次。

两个前提：

- 手机和电脑在同一局域网（同一 WiFi / 电脑热点）；
- 电脑防火墙放行（面板里的「一键放行」现在会把 **TCP 17650-17655 + UDP** 一起放行）；
- iOS 首次广播会弹「本地网络」权限，必须点允许（`Info.plist` 里的 `NSLocalNetworkUsageDescription`，CI 自动注入）。

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

注意：桌面上跑要能收到 UDP 应答，电脑端的手机服务得先开着。

## 已知限制

- 只能在**同一子网**发现（跨网段/路由器隔离不行）；
- 局域网 http 明文靠 ATS 放行，正式分发前可以换成自签 https（可选）。
