@echo off
chcp 936 >nul
title 批量 PDF 打印工具 - 只出 exe（快）

cd /d "%~dp0"

echo ==================================================
echo   只出 exe：不打包 NSIS/MSI、不做便携 zip
echo   目录: %CD%
echo ==================================================
echo.

where pnpm >nul 2>nul
if errorlevel 1 (
  echo [错误] 找不到 pnpm。请先安装 Node 22+ 和 pnpm。
  goto :fail
)

if not exist "node_modules" (
  echo [提示] 没有 node_modules，先执行 pnpm install ...
  call pnpm install
  if errorlevel 1 goto :fail
  echo.
)

tasklist /fi "imagename eq desktop.exe" 2>nul | findstr /i "desktop.exe" >nul
if not errorlevel 1 (
  echo [错误] desktop.exe 正在运行，请先关闭程序再打包。
  goto :fail
)

rem ---------- sccache：Rust 编译缓存，程序在 D 盘，缓存目录也在 D 盘 ----------
set "RUSTC_WRAPPER=D:\tools\sccache\sccache.exe"
set "SCCACHE_DIR=D:\sccache-cache"
if exist "%RUSTC_WRAPPER%" (
  if not exist "%SCCACHE_DIR%" mkdir "%SCCACHE_DIR%"
  "%RUSTC_WRAPPER%" --start-server >nul 2>nul
  echo [sccache] 已启用  %RUSTC_WRAPPER%
) else (
  echo [sccache] 没找到 %RUSTC_WRAPPER%，这次不缓存
  set "RUSTC_WRAPPER="
)
echo.

for /f %%i in ('powershell -NoProfile -Command "[int][double]::Parse((Get-Date -UFormat %%s))"') do set "T0=%%i"

echo [1/1] 前端构建 + Rust 编译（不做安装包）...
call pnpm tauri build --no-bundle %*
if errorlevel 1 goto :fail

for /f %%i in ('powershell -NoProfile -Command "[int][double]::Parse((Get-Date -UFormat %%s))"') do set "T1=%%i"
set /a DT=%T1%-%T0%

echo.
echo 完成，耗时 %DT% 秒
for %%F in ("src-tauri\target\release\desktop.exe") do echo   exe: %%~fF  ^(%%~zF 字节^)
echo.
echo 要安装包 / 便携 zip 就跑 build-release.bat
exit /b 0

:fail
echo.
echo ==================================================
echo   打包失败，请查看上方的错误信息。
echo ==================================================
pause
exit /b 1
