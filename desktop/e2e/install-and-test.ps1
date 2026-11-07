param([Parameter(Mandatory=$true)][string]$Installer, [string]$DiagnosticExe, [switch]$StartupOnly)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$nodeExecutable = (Get-Command node.exe).Source
$pythonExecutable = (Get-Command python.exe).Source
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$runDir = Join-Path $repo ".tmp\installer-acceptance-$stamp"
$installDir = Join-Path $runDir 'installed'
$appDataPath = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'com.hanlin.pdfprint'
$backupDataPath = "$appDataPath.acceptance-backup-$stamp"
$registryKeys = @('HKCU\Software\hanlin\PdfBatchPrint', 'HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\PdfBatchPrint')
$backups = @()
$dataMoved = $false
$appDataIsolated = $false
$installed = $false
$testPassed = $false
$registryTouched = $false
$originalInstall = Join-Path $env:LOCALAPPDATA 'PdfBatchPrint\desktop.exe'
$originalExeHash = if (Test-Path -LiteralPath $originalInstall) { (Get-FileHash -LiteralPath $originalInstall).Hash } else { $null }
New-Item -ItemType Directory -Path $runDir | Out-Null
$runDir | Set-Content -LiteralPath (Join-Path $repo '.tmp\last-installer-acceptance.txt')
if (Get-Process desktop -ErrorAction SilentlyContinue) { throw '请先关闭现有桌面程序，避免测试期间占用数据库' }
foreach ($key in $registryKeys) {
  $file = Join-Path $runDir ("registry-" + $backups.Count + '.reg')
  & reg.exe export $key $file /y 2>$null | Out-Null
  $backups += @{ Key=$key; File=$file; Exists=($LASTEXITCODE -eq 0) }
}
try {
  $resolvedInstaller = (Resolve-Path -LiteralPath $Installer).Path
  if (-not $resolvedInstaller.StartsWith((Join-Path $repo 'desktop\src-tauri\target\release') + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Installer outside expected release directory' }
  $registryTouched = $true
  $process = Start-Process -FilePath $resolvedInstaller -ArgumentList @('/S','/NS',"/D=$installDir") -WindowStyle Hidden -Wait -PassThru
  if ($process.ExitCode -ne 0) { throw "Installer exited: $($process.ExitCode)" }
  if (-not (Test-Path -LiteralPath (Join-Path $installDir 'desktop.exe'))) { throw 'Installed executable is missing' }
  if ($DiagnosticExe) {
    $diagnosticPath = (Resolve-Path -LiteralPath $DiagnosticExe).Path
    if (-not $diagnosticPath.StartsWith((Join-Path $repo 'desktop\src-tauri\target\release') + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Diagnostic executable outside release directory' }
    Copy-Item -LiteralPath $diagnosticPath -Destination (Join-Path $installDir 'desktop.exe') -Force
    Write-Output 'DIAGNOSTIC: Using rebuilt executable; this run does not certify the original installer binary'
  }
  $installed = $true
  Write-Output 'PASS: NSIS silent install into isolated directory'
  foreach ($file in @('PP-OCRv6_medium_det.mnn','PP-OCRv6_small_rec.mnn','ppocr_keys_v6_small.txt')) {
    $expected = (Get-FileHash -LiteralPath (Join-Path $repo "desktop\models\$file")).Hash
    $actual = (Get-FileHash -LiteralPath (Join-Path $installDir "models\$file")).Hash
    if ($expected -ne $actual) { throw "Installed model hash mismatch: $file" }
  }
  Write-Output 'PASS: Installed model hashes match release models'
  if (Test-Path -LiteralPath $appDataPath) {
    if (Test-Path -LiteralPath $backupDataPath) { throw 'Data backup destination already exists' }
    $resolvedData = (Resolve-Path -LiteralPath $appDataPath).Path
    $expectedData = Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'com.hanlin.pdfprint'
    if ($resolvedData -ne $expectedData) { throw 'Unexpected application data directory' }
    Move-Item -LiteralPath $resolvedData -Destination $backupDataPath
    $dataMoved = $true
  }
  New-Item -ItemType Directory -Path $appDataPath | Out-Null
  $appDataIsolated = $true
  # 临时测试目录沿用原目录的权限；原目录及其 ACL 保存在备份中不变。
  if ($dataMoved) { Set-Acl -LiteralPath $appDataPath -AclObject (Get-Acl -LiteralPath $backupDataPath) }
  & $pythonExecutable -c "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute('PRAGMA user_version=0'); c.close()" (Join-Path $appDataPath 'pdf-tool.db')
  if ($LASTEXITCODE -ne 0) { throw 'Cannot initialize isolated empty database' }
  New-Item -ItemType Directory -Path (Join-Path $appDataPath 'logs') -Force | Out-Null
  New-Item -ItemType File -Path (Join-Path $appDataPath 'logs\app.log') -Force | Out-Null

  $env:ACCEPTANCE_RUN = $runDir
  $env:ACCEPTANCE_EXE = Join-Path $installDir 'desktop.exe'
  $env:ACCEPTANCE_PLAYWRIGHT = Join-Path $repo '.tmp\acceptance-deps\node_modules\playwright-core'
  $env:ACCEPTANCE_EDGE = 'C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe'
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=19223 --remote-debugging-address=127.0.0.1'
  $env:WEBVIEW2_USER_DATA_FOLDER = Join-Path $runDir 'webview-profile'
  $env:PATH = (Join-Path $repo '.tmp\acceptance-bin') + ';' + $env:PATH
  $phases = if ($StartupOnly) { @('startup') } else { @('primary','restart','missing-models') }
  foreach ($phase in $phases) {
    $env:ACCEPTANCE_PHASE = $phase
    if ($phase -eq 'missing-models') {
      $modelSource = (Resolve-Path -LiteralPath (Join-Path $installDir 'models')).Path
      if ($modelSource -ne (Join-Path $installDir 'models')) { throw 'Unexpected model source directory' }
      Move-Item -LiteralPath $modelSource -Destination (Join-Path $installDir 'models-disabled')
    }
    & $nodeExecutable (Join-Path $PSScriptRoot 'installer-acceptance.cjs')
    if ($LASTEXITCODE -ne 0) { throw "Acceptance phase failed: $phase" }
  }
  $disabledModels = Join-Path $installDir 'models-disabled'
  if (Test-Path -LiteralPath $disabledModels) {
    if (-not $disabledModels.StartsWith($runDir + '\')) { throw 'Models outside test workspace' }
    Move-Item -LiteralPath $disabledModels -Destination (Join-Path $installDir 'models')
  }
  # 仅对隔离安装运行卸载器；正式安装路径和数据在最终清理时恢复。
  if (-not $installDir.StartsWith($runDir + '\')) { throw 'Uninstall target outside test workspace' }
  $uninstall = Start-Process -FilePath (Join-Path $installDir 'uninstall.exe') -ArgumentList @('/S','/UPDATE',"_?=$installDir") -WindowStyle Hidden -Wait -PassThru
  if ($uninstall.ExitCode -ne 0 -or (Test-Path -LiteralPath (Join-Path $installDir 'desktop.exe'))) { throw 'Isolated uninstall failed' }
  Write-Output 'PASS: Isolated NSIS uninstall'
  $testPassed = $true
} finally {
  # 不按进程名停止其他实例，仅清理本次隔离安装的程序。
  Get-CimInstance Win32_Process -Filter "Name='desktop.exe'" | Where-Object { $_.ExecutablePath -eq (Join-Path $installDir 'desktop.exe') } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  if ($appDataIsolated -and (Test-Path -LiteralPath $appDataPath)) {
    if ($appDataPath -ne (Join-Path ([Environment]::GetFolderPath('ApplicationData')) 'com.hanlin.pdfprint')) { throw 'Unexpected test data path' }
    Move-Item -LiteralPath $appDataPath -Destination (Join-Path $runDir 'test-appdata')
  }
  if ($dataMoved) {
    if (Test-Path -LiteralPath $appDataPath) { throw "Cannot restore app data; original backup retained at $backupDataPath" }
    Move-Item -LiteralPath $backupDataPath -Destination $appDataPath
    Write-Output 'RESTORED: Original application data'
  }
  if ($registryTouched) {
    foreach ($backup in $backups) {
      & reg.exe delete $backup.Key /f 2>$null | Out-Null
      if ($backup.Exists) {
        & reg.exe import $backup.File | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Registry restore failed: $($backup.Key)" }
      }
    }
    Write-Output 'RESTORED: Original installation registry'
  }
  if ($originalExeHash -and (Get-FileHash -LiteralPath $originalInstall).Hash -ne $originalExeHash) { throw 'Original installed executable changed unexpectedly' }
  @{ passed=$testPassed; installed=$installed; originalDataRestored=$dataMoved; originalExecutableUnchanged=$true; runDirectory=$runDir } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $runDir 'installation-result.json')
  Write-Output "REPORT: $runDir"
}
if (-not $testPassed) { exit 1 }
