# One-line installer for DataBug on Windows (PowerShell):
#   irm https://raw.githubusercontent.com/hayes-roach/DataBug/main/install.ps1 | iex
# Downloads the latest setup and installs DataBug for the current user
# (no admin rights needed), with a Start menu shortcut and an uninstaller
# in Settings > Apps. Set $env:DATABUG_VERSION = "v0.2.0" to pick a release.
$ErrorActionPreference = 'Stop'
$repo = 'hayes-roach/DataBug'
$version = if ($env:DATABUG_VERSION) { $env:DATABUG_VERSION } else { 'latest' }
$base = if ($version -eq 'latest') { "https://github.com/$repo/releases/latest/download" } else { "https://github.com/$repo/releases/download/$version" }
$setup = Join-Path $env:TEMP 'DataBug-setup.exe'

Write-Host 'Downloading DataBug for Windows...' -ForegroundColor Cyan
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
$ProgressPreference = 'SilentlyContinue'
Invoke-WebRequest -Uri "$base/DataBug-windows-x86_64-setup.exe" -OutFile $setup -UseBasicParsing

Write-Host 'Installing...' -ForegroundColor Cyan
$p = Start-Process -FilePath $setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', '/TASKS=desktopicon' -Wait -PassThru
Remove-Item $setup -ErrorAction SilentlyContinue
if ($p.ExitCode -ne 0) { throw "The DataBug installer failed (exit code $($p.ExitCode))" }

$exe = Join-Path $env:LOCALAPPDATA 'Programs\DataBug\DataBug.exe'
Write-Host 'DataBug installed. Find it in the Start menu.' -ForegroundColor Green
if (Test-Path $exe) { Start-Process $exe }
