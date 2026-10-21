@echo off
chcp 936 >nul
title 批量 PDF 打印工具 - 开发模式
cd /d "%~dp0"

echo ==================================================
echo   批量 PDF 打印工具 - 开发模式
echo   目录: %CD%
echo ==================================================
echo.

rem ---------- 环境检查 ----------
where pnpm >nul 2>nul
if errorlevel 1 goto :no_pnpm

where cargo >nul 2>nul
if errorlevel 1 goto :no_cargo

if exist "node_modules" goto :deps_ok
echo [提示] 没有 node_modules，先执行 pnpm install ...
call pnpm install
if errorlevel 1 goto :fail
echo.
:deps_ok

rem ---------- 端口检查（vite 固定 1420）----------
netstat -ano | findstr ":1420" | findstr LISTENING >nul
if errorlevel 1 goto :port_ok
echo [警告] 端口 1420 已被占用，通常意味着另一个 dev 窗口还开着。
echo         继续启动会因为端口冲突失败。
set "GO="
set /p "GO=仍要继续？[Y/N] "
if /i not "%GO%"=="Y" goto :end
:port_ok

rem ---------- 旧进程提示（不阻塞，只提醒）----------
tasklist /fi "imagename eq desktop.exe" 2>nul | find /i "desktop.exe" >nul
if errorlevel 1 goto :proc_ok
echo [提示] 检测到 desktop.exe 正在运行。
echo         如果它是旧的 dev 实例，会锁住 target\debug\desktop.exe 让编译失败；
echo         如果只是打包好的正式版，不影响本次开发。
echo.
:proc_ok

rem ---------- 启动 ----------
set "PDFPRINT_LOG=debug"
echo 启动中：首次要编译 Rust（约 1-2 分钟），之后改前端是秒级热更新。
echo 日志：%%APPDATA%%\com.hanlin.pdfprint\logs\app.log（本窗口级别 debug）
echo 关闭窗口或按 Ctrl+C 结束。
echo.
call pnpm tauri dev %*
set "CODE=%ERRORLEVEL%"
echo.
if not "%CODE%"=="0" echo [提示] 退出码 %CODE% —— 若是编译错误，请向上翻日志。
if "%CODE%"=="0" echo 已退出。
pause
exit /b %CODE%

:no_pnpm
echo [错误] 找不到 pnpm。请先安装 Node 22+ 和 pnpm。
goto :fail

:no_cargo
echo [错误] 找不到 cargo。请先安装 Rust（MSVC 工具链）。
goto :fail

:fail
echo.
echo 启动失败，请查看上方的错误信息。
pause
exit /b 1

:end
echo 已取消。
pause
exit /b 0
