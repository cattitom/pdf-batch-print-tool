# 出货助手（iOS 壳）

手机打开后先进入本地「工作台」，无需连接电脑。入库 / 出货是工作台中的一个模块，点击后才发现并连接电脑。

- 入库 / 出货：连接电脑提供的局域网清单；连接失败可返回工作台重试。
- 本地清单：无需电脑，查看手机上次保存的清单；首次使用没有快照时显示引导。
- 连接设置：未连接时也能管理配对电脑和历史。

电脑端负责实时业务数据；手机工作台、设置和离线快照保存在手机端。

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

注意：桌面上跑要能收到 UDP 应答，电脑端的手机服务得先开着。

## 已知限制

- 只能在**同一子网**发现（跨网段/路由器隔离不行）；
- 局域网 http 明文靠 ATS 放行，正式分发前可以换成自签 https（可选）。
