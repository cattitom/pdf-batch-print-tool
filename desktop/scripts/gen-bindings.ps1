# 重新生成前端 TypeScript 契约（ts-rs）
#
# 改了 Rust 侧的 DTO（commands.rs）或核心类型（PdfInfo / PrintHistoryEntry /
# LabelDocEntry / LabelSku）之后跑一次，src/bindings/*.ts 会自动更新。
# 生成物不要手改。

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$env:TS_RS_EXPORT_DIR = Join-Path $root "src\bindings"

Push-Location $root
try {
    Write-Host "==> core"
    cargo test --manifest-path core\Cargo.toml --release export_bindings
    if ($LASTEXITCODE -ne 0) { throw "core 类型导出失败" }

    Write-Host "==> src-tauri"
    cargo test --manifest-path src-tauri\Cargo.toml --release export_bindings
    if ($LASTEXITCODE -ne 0) { throw "桥接层类型导出失败" }
}
finally {
    Pop-Location
}

$out = Join-Path $root "src\bindings"
$count = (Get-ChildItem $out -Filter *.ts).Count
Write-Host "完成：$count 个类型文件已写入 $out"
