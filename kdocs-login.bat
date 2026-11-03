@echo off
setlocal EnableExtensions
title WPS login - scan the QR code, then back to the app
set "LOG=%TEMP%\kdocs-login.log"
echo [%DATE% %TIME%] start > "%LOG%"

rem Dedicated browser profile: the WPS login state lives here, the sync reuses it
set "PROFILE=%LOCALAPPDATA%\kdocs-cdp-profile"
set "URL=https://www.kdocs.cn/l/cie4FmnjnBhW"

rem Chrome may be in Program Files / Program Files (x86) / current user only
set "CHROME=%KDOCS_CHROME%"
if not defined CHROME set "CHROME=%ProgramFiles%\Google\Chrome\Application\chrome.exe"
if not exist "%CHROME%" set "CHROME=%ProgramFiles(x86)%\Google\Chrome\Application\chrome.exe"
if not exist "%CHROME%" set "CHROME=%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe"
if not exist "%CHROME%" goto nochrome

echo.
echo   Profile    : %PROFILE%
echo   Debug port : 127.0.0.1:9222
echo.
echo   1^) In the Chrome window, scan the QR code and sign in to WPS (only once)
echo   2^) Back in the app: "Shipping list" page -^> "Sync from cloud sheet" button
echo   3^) Keep this window open, or close it - the login state is saved either way
echo.
echo Opening Chrome...
echo [%DATE% %TIME%] chrome = "%CHROME%" >> "%LOG%"
start "" "%CHROME%" --remote-debugging-port=9222 --user-data-dir="%PROFILE%" --no-first-run --no-default-browser-check --disable-breakpad "%URL%"
echo [%DATE% %TIME%] start issued, errorlevel=%ERRORLEVEL% >> "%LOG%"
echo.
echo If Chrome did not open, send this log to the maintainer: %LOG%
echo.
echo Press any key to close this window (Chrome and the login stay).
pause >nul
exit /b 0

:nochrome
echo.
echo Chrome not found. Install Google Chrome, or edit the CHROME= line in this file:
echo   set "CHROME=C:\Program Files\Google\Chrome\Application\chrome.exe"
echo.
echo [%DATE% %TIME%] no chrome found >> "%LOG%"
pause
exit /b 1
