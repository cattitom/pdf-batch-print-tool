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
tasklist /fi "imagename eq desktop.exe" 2>nul | find /i "desktop.exe" >nul
if not errorlevel 1 (
  echo [错误] desktop.exe 正在运行，请先关闭程序再打包。
  goto :fail
)

rem ---------- 3. 打包 ----------
echo [1/2] 前端构建 + Rust 编译 + 安装包（LTO 全量编译，约 3-8 分钟）...
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
set "OPEN="
set /p "OPEN=要打开产物目录吗？[Y/N] "
if /i "%OPEN%"=="Y" if exist "%REL%\bundle" explorer "%REL%\bundle"

echo.
echo 全部完成。
pause
exit /b 0

:fail
echo.
echo ==================================================
echo   打包失败，请查看上方的错误信息。
echo   常见原因：程序未关闭 / 网络下载依赖失败 / 磁盘空间不足。
echo ==================================================
pause
exit /b 1
