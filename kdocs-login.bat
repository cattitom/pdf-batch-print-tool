@echo off
setlocal
title kdocs login - keep this window open
set "LOG=%TEMP%\kdocs-login.log"
echo [%DATE% %TIME%] start > "%LOG%"

set "PROFILE=%LOCALAPPDATA%\kdocs-cdp-profile"
set "URL=https://www.kdocs.cn/l/cie4FmnjnBhW"
set "CHROME=%KDOCS_CHROME%"
if not defined CHROME set "CHROME=C:\Program Files\Google\Chrome\Application\chrome.exe"
if not exist "%CHROME%" set "CHROME=C:\Program Files (x86)\Google\Chrome\Application\chrome.exe"
rem Chrome may also be installed per-user (AppData\Local instead of Program Files)
if not exist "%CHROME%" set "CHROME=%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe"
if not exist "%CHROME%" goto nochrome

echo.
echo   Profile  : %PROFILE%
echo   DebugPort: 127.0.0.1:9222
echo.
echo   1) scan the QR code and log in
echo   2) keep this window open
echo   3) tell the agent when done
echo.
echo Starting Chrome...
echo [%DATE% %TIME%] chrome= "%CHROME%" >> "%LOG%"
start "" "%CHROME%" --remote-debugging-port=9222 --user-data-dir="%PROFILE%" --no-first-run --no-default-browser-check --disable-breakpad "%URL%"
echo [%DATE% %TIME%] start issued, errorlevel=%ERRORLEVEL% >> "%LOG%"
echo.
echo If Chrome did NOT open, open this log file and send it to the agent:
echo   %LOG%
echo.
echo Press any key to close this window (Chrome keeps running).
pause >nul
exit /b 0

:nochrome
echo Chrome not found. Edit the CHROME= line in this bat file.
echo [%DATE% %TIME%] no chrome found >> "%LOG%"
pause
exit /b 1
