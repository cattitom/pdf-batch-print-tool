@echo off
chcp 936 >nul
cd /d "%~dp0"
where node >nul 2>nul
if errorlevel 1 (
  echo 请先安装 Node.js 22.12.0 或更高版本。
  pause
  exit /b 1
)
node scripts/check-runtime.mjs
if errorlevel 1 goto :fail
if not exist "node_modules" (
  call pnpm install
  if errorlevel 1 goto :fail
)
node scripts/release.mjs %*
if errorlevel 1 goto :fail
set "INSTALLER_DIR=%CD%\src-tauri\target\release\installers\PdfBatchPrint"
echo.
echo Installer folder: %INSTALLER_DIR%
start "" explorer "%INSTALLER_DIR%"
pause
exit /b 0
:fail
echo 构建失败，请查看上方错误信息。
pause
exit /b 1
