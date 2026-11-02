# 出货助手（iOS 壳）

电脑上的出货清单跑在局域网里（`http://<电脑IP>:17650/?t=<口令>`）。这个目录是个**极小的 Tauri 应用**，
只做一件事：**打开就自动找到电脑、全屏显示那个页面**。没有后端、没有 OCR，所以能在 iOS 上编。

## 为什么不做成完整 App

桌面那份程序依赖 Windows 专属的东西（MNN 预编译的 OCR、WPS 注册表、系统打印），iOS 上编不过；
而且手机真正需要的只是「看票 / 领取 / 打勾」。所以这里做的是**瘦客户端**：数据、逻辑全在电脑端。

## 自动发现怎么工作

打开 App 先找电脑，找到了不直接跳，先问一句要不要连：

1. **Bonjour 优先**：电脑端（`desktop/src-tauri/src/mobile.rs`）开手机服务时注册 `_shippingshell._tcp`，
   TXT 里带 `token` 和电脑名；App 用系统 `DNSServiceBrowse` 找（mDNSResponder 代发组播，
   绕开 iOS 对 App 自己广播的限制）。
2. **广播兜底**：没走通就发 UDP 暗号 `PDF-MOBILE-DISCOVER` 到 `255.255.255.255:17655`（6 轮 ≈ 3 秒），
   电脑回 `{port, token, name}`，用回包源 IP 拼地址。
3. **找到了**：显示「发现电脑 · 电脑名 IP」，带一个 **「下次直接连接，不再询问」开关**；
   勾上再点连接就把这台记到本地，以后启动发现到它就直接连，不再弹卡片。
4. **没找到**：进「没找到电脑」页 —— 主按钮是 **扫二维码**（原生相机，扫电脑面板上的二维码），
   下面是 **最近连过**（最多 5 台，点一下用单播重新问口令再连）和手填 IP 兜底。

历史记录只存在手机本地（`localStorage`），记 IP / 电脑名 / 最后连接时间 / 上次的整条链接；
连接时优先重新问一次口令，换过口令也不会拿着旧 token 打不开。

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

注意：桌面上跑要能收到 UDP 应答，电脑端的手机服务得先开着。

## 已知限制

- 只能在**同一子网**发现（跨网段/路由器隔离不行）；
- 局域网 http 明文靠 ATS 放行，正式分发前可以换成自签 https（可选）。
