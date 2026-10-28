# OCR 模型（PaddleOCR / MNN）

「录货清单」页签拖截图时，会优先用这里的模型；**没有模型就自动回退到 Windows 自带 OCR**。

## 需要的文件

程序会自动挑一套（按顺序，找到哪套用哪套）：

| 优先级 | 检测 | 识别 | 字典 | 说明 |
|---|---|---|---|---|
| 1 | `PP-OCRv6_small_det.mnn` | `PP-OCRv6_small_rec.mnn` | `ppocr_keys_v6_small.txt` | **推荐**，小字更准 |
| 2 | `PP-OCRv5_mobile_det.mnn` | `PP-OCRv5_mobile_rec.mnn` | `ppocr_keys_v5.txt` | 更小更快 |

来源：`zibo-chen/ocr-rs` 仓库的 `models/` 目录（MNN 格式，可直接用，不需要自己转换）。

```powershell
$base = 'https://raw.githubusercontent.com/zibo-chen/ocr-rs/next/models/'
cd <repo>\desktop\models
foreach ($f in 'PP-OCRv6_small_det.mnn','PP-OCRv6_small_rec.mnn','ppocr_keys_v6_small.txt') {
    Invoke-WebRequest ($base + $f) -OutFile $f
}
```

想再准可以换 `PP-OCRv6_medium`（31MB + 38MB，慢一些），把文件放进来自动生效，代码不用改。

## 放在哪

- 开发：`desktop/models/`（就是本目录）
- 打包后：exe 同级 `models/`，或 `resources/models/`

模型不进版本库（21MB 二进制），release 时单独拷。
