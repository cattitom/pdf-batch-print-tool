@echo off
chcp 936 >nul
title 批量 PDF 打印工具 - Release 打包
cd /d "%~dp0"

echo ==================================================
echo   批量 PDF 打印工具 - Release 打包
echo   目录: %CD%
echo ==================================================
echo.

rem ---------- 1. 环境检查 ----------
where pnpm >nul 2>nul
if errorlevel 1 (
  echo [错误] 找不到 pnpm。请先安装 Node 22+ 和 pnpm。
  goto :fail
)

where cargo >nul 2>nul
if errorlevel 1 (
  echo [错误] 找不到 cargo。请先安装 Rust（MSVC 工具链）。
  goto :fail
)

if not exist "node_modules" (
  echo [提示] 没有 node_modules，先执行 pnpm install ...
  call pnpm install
  if errorlevel 1 goto :fail
  echo.
)

rem ---------- 2. 程序是否正在运行（占用 exe 会让打包失败）----------
tasklist /fi "imagename eq desktop.exe" 2>nul | findstr /i "desktop.exe" >nul
if not errorlevel 1 (
  echo [错误] desktop.exe 正在运行，请先关闭程序再打包。
  goto :fail
)

rem ---------- sccache：Rust 编译缓存（程序放 D 盘，缓存也放 D 盘） ----------
set "RUSTC_WRAPPER=D:\tools\sccache\sccache.exe"
set "SCCACHE_DIR=D:\sccache-cache"
if exist "%RUSTC_WRAPPER%" (
  if not exist "%SCCACHE_DIR%" mkdir "%SCCACHE_DIR%"
  "%RUSTC_WRAPPER%" --start-server >nul 2>nul
  echo [sccache] 已启用  %RUSTC_WRAPPER%
) else (
  echo [sccache] 没找到，这次不缓存
  set "RUSTC_WRAPPER="
)
echo.

rem ---------- 3. 打包 ----------
echo [1/2] 前端构建 + Rust 编译 + 安装包（thin LTO：改代码增量约 1.5-2 分钟；首次或换依赖 5-8 分钟）...
echo       默认只出 NSIS 安装包；额外参数透传，例如: build-release.bat --bundles msi
echo.
call pnpm tauri build %*
if errorlevel 1 goto :fail

rem ---------- 4. 结果 ----------
echo.
echo [2/2] 打包完成，产物：
set "REL=src-tauri\target\release"
if exist "%REL%\desktop.exe" for %%F in ("%REL%\desktop.exe") do echo   绿色单文件 : %%~fF  ^(%%~zF 字节^)
if exist "%REL%\bundle\nsis\*.exe" for %%F in ("%REL%\bundle\nsis\*.exe") do echo   NSIS 安装包: %%~fF  ^(%%~zF 字节^)
if exist "%REL%\bundle\msi\*.msi" for %%F in ("%REL%\bundle\msi\*.msi") do echo   MSI 安装包 : %%~fF  ^(%%~zF 字节^)

echo.
rem ---------- 5. 便携版（免安装 zip，绕开 NSIS 临时目录）----------
echo.
echo [3/3] 生成便携版 zip（解压即用，不经过安装器）...
set "PORTABLE=%REL%\portable\PdfBatchPrint"
if exist "%REL%\portable" rmdir /s /q "%REL%\portable"
mkdir "%PORTABLE%\models"
copy /y "%REL%\desktop.exe" "%PORTABLE%\desktop.exe" >nul
copy /y "models\PP-OCRv6_medium_det.mnn" "%PORTABLE%\models\" >nul
copy /y "models\PP-OCRv6_small_rec.mnn" "%PORTABLE%\models\" >nul
copy /y "models\ppocr_keys_v6_small.txt" "%PORTABLE%\models\" >nul
>"%PORTABLE%\使用说明.txt" echo 解压到任意目录，双击 desktop.exe 即可。
>>"%PORTABLE%\使用说明.txt" echo.
>>"%PORTABLE%\使用说明.txt" echo models 文件夹必须和 exe 放在一起（OCR 模型）。
powershell -NoProfile -Command "Compress-Archive -Path '%PORTABLE%\*' -DestinationPath '%REL%\PdfBatchPrint-portable.zip' -Force"
if exist "%REL%\PdfBatchPrint-portable.zip" echo   便携版 zip : %REL%\PdfBatchPrint-portable.zip

rem open output folder by default (no prompt, per user request)
if exist "%REL%\bundle" (start "" explorer "%REL%\bundle") else if exist "%REL%" (start "" explorer "%REL%")

echo.
echo 全部完成。
exit /b 0

:fail
echo.
echo ==================================================
echo   打包失败，请查看上方的错误信息。
echo   常见原因：程序未关闭 / 网络下载依赖失败 / 磁盘空间不足。
echo ==================================================
pause
exit /b 1
