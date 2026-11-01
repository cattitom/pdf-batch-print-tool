# 出货助手（iOS 壳）

电脑上的出货清单跑在局域网里（`http://<电脑IP>:17650/?t=<口令>`）。这个目录是个**极小的 Tauri 应用**，
只做一件事：填地址 → 全屏 iframe 显示那个页面。没有后端、没有 OCR，所以能在 iOS 上编。

## 为什么不做成完整 App

桌面那份程序依赖 Windows 专属的东西（MNN 预编译的 OCR、WPS 注册表、系统打印），iOS 上编不过；
而且手机真正需要的只是「看票 / 领取 / 打勾」。所以这里做的是**瘦客户端**。

## 出 IPA（不需要 Mac）

1. 推到 GitHub（仓库：`cattitom/pdf-batch-print-tool`）；
2. Actions → **iOS 出货助手 IPA** → Run workflow；
3. 跑完在 Summary 下方下载 `shipping-shell-ipa`，里面是 `shipping-shell-unsigned.ipa`。

CI 做的事：`tauri ios init` 生成 Xcode 工程 → 注入 ATS（允许 http 局域网）→ `xcodebuild` 不签名编译 →
把 `.app` 塞进 `Payload/` 打成 `.ipa`。**产物未签名**，用你自己的证书签（AltStore / 爱思 / `codesign` 都行）。

## 本地调试桌面版

```bash
cd ios-shell/src-tauri
cargo run            # 会弹出 390×844 的窗口，跟手机上长得一样
```

## 已知限制

- 第一次 CI 大概率要调 scheme 名 / Xcode 版本，失败就把日志贴出来；
- App 里目前是**手动粘贴链接**（电脑上「手机出货 → 复制链接」），还没做扫码；
- 局域网 http 明文靠 ATS 放行，正式分发前可以换成自签 https（可选）。
