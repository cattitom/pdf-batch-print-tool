param([Parameter(Mandatory=$true)][string]$Source, [Parameter(Mandatory=$true)][string]$Archive)
$ErrorActionPreference = 'Stop'
Compress-Archive -LiteralPath $Source -DestinationPath $Archive -Force
