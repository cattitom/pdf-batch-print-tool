# OCR 模型（PaddleOCR / MNN）

「录货清单」页签拖截图时，会优先用这里的模型；**没有模型就自动回退到 Windows 自带 OCR**。

## 需要的文件

程序按下面的顺序挑第一套文件齐全的档位（精度从高到低）：

| 顺序 | 检测 | 识别 | 字典 | 说明 |
|---|---|---|---|---|
| 1 | `PP-OCRv6_medium_det.mnn` | `PP-OCRv6_small_rec.mnn` | `ppocr_keys_v6_small.txt` | **实际在用**：medium 检测框更稳，small 识别对窄列更稳。三张真截图全对 |
| 2 | `PP-OCRv6_medium_det.mnn` | `PP-OCRv6_medium_rec.mnn` | `ppocr_keys_v6_medium.txt` | 纯 medium（官方 accuracy-first）。实测第三张截图会把「11」读成「1」 |
| 3 | `PP-OCRv6_small_det.mnn` | `PP-OCRv6_small_rec.mnn` | `ppocr_keys_v6_small.txt` | 均衡档 |
| 4 | `PP-OCRv5_mobile_det.mnn` | `PP-OCRv5_mobile_rec.mnn` | `ppocr_keys_v5.txt` | 老轻量档，兜底 |

v6 的字典按档位分开命名（`tiny` / `small` / `medium` 各一份）。实测 v6 `small` 和 `medium`
的字典内容完全一样（md5 `68C581DF…`），但文件名仍按档位区分，不要改名。

来源：`zibo-chen/ocr-rs` 仓库的 `models/` 目录（MNN 格式，可直接用，不需要自己转换）。

```powershell
$base = 'https://raw.githubusercontent.com/zibo-chen/ocr-rs/next/models/'
cd <repo>\desktop\models
foreach ($f in 'PP-OCRv6_medium_det.mnn','PP-OCRv6_small_rec.mnn','ppocr_keys_v6_small.txt') {
    Invoke-WebRequest ($base + $f) -OutFile $f
}
```

第一组约 40MB（29.6 + 10.2 + 0.1）。想试纯 medium 再补 `PP-OCRv6_medium_rec.mnn`（36.6MB）。

## 运行时参数

`src-tauri/src/ocr.rs` 的 `build_engine()` 已按最高精度配置：

| 参数 | 值 | 为什么 |
|---|---|---|
| `precision_mode` | `High` | MNN 关掉低精度加速，数字和细笔画更稳 |
| `thread_count` | 逻辑核数（上限 20） | 本机 20 线程跑满 |
| `min_result_confidence` | 0.2 | 默认 0.5 会吞掉标记列单个「1」 |
| `det_options.max_side_len` | 4096 | 默认 960 把 1600+ 宽截图缩一半，小字糊掉 |
| `det_options.box_threshold` / `score_threshold` | 0.4 / 0.25 | 放低召回，细表格线里的字不漏 |

`unclip_ratio`、`box_border`、`rec.target_height` 保持默认：试过调大反而更差（第三张合计从 118 掉到 117）。

## 自动降级

`parse_image()` 会按上表逐档尝试，**合计对不上就换下一档重跑**，取能对上「合计 N 件」的那份。
如果表里没写合计（`expected_qty = None`），第一档就是结果，没有降级依据。

## 放在哪

- 开发：`desktop/models/`（就是本目录）
- 打包后：exe 同级 `models/`，或 `resources/models/`
- `tauri.conf.json` 的 `bundle.resources` 已经把第一组打进安装包（约 40MB）

模型不进版本库（`*.mnn` 在 `.gitignore` 里），release 时靠打包配置带出去。
