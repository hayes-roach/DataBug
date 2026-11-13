# One-line installer and updater for DataBug on Windows (PowerShell):
#   irm https://raw.githubusercontent.com/hayes-roach/DataBug/main/install.ps1 | iex
#
# Installs the latest release, or updates an existing install in place
# (connections, settings and history are kept). A new install goes to your
# user account (no admin rights needed); an existing install is updated where
# it is, asking for admin rights only if it was installed for all users.
#
# Options (set before running):
#   $env:DATABUG_VERSION = "v0.3.0"   install that release instead of the latest
#   $env:DATABUG_FORCE = "1"          reinstall even if already up to date

function Install-DataBug {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

    $repo = 'hayes-roach/DataBug'
    $appId = '{6F0C1D8E-3B7A-4E52-9C1D-DA7ABA6B0001}_is1'
    $requested = $env:DATABUG_VERSION
    $base = if ($requested) { "https://github.com/$repo/releases/download/$requested" } else { "https://github.com/$repo/releases/latest/download" }

    function Say($text, $color = 'Cyan') { Write-Host $text -ForegroundColor $color }
    function Fail($text) { Write-Host $text -ForegroundColor Red }

    # --- what's installed now ----------------------------------------------
    $installed = $null
    $keys = @(
        @{ Path = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$appId"; AllUsers = $false },
        @{ Path = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$appId"; AllUsers = $true },
        @{ Path = "HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\$appId"; AllUsers = $true }
    )
    foreach ($k in $keys) {
        $entry = Get-ItemProperty -Path $k.Path -ErrorAction SilentlyContinue
        if ($entry) {
            $installed = @{ Version = $entry.DisplayVersion; Location = $entry.InstallLocation; AllUsers = $k.AllUsers }
            break
        }
    }

    # --- which version we're installing --------------------------------------
    $target = $null
    if ($requested) {
        $target = $requested.TrimStart('v', 'V')
    } else {
        try {
            $latest = Invoke-RestMethod -Uri "https://api.github.com/repos/$repo/releases/latest" -UseBasicParsing -TimeoutSec 30 -Headers @{ 'User-Agent' = 'DataBug-installer' }
            $target = "$($latest.tag_name)".TrimStart('v', 'V')
        } catch {
            $target = $null  # still install; we just can't say which version
        }
    }

    if ($installed -and $target -and $installed.Version -eq $target -and -not $env:DATABUG_FORCE) {
        Say "DataBug $target is already installed and up to date." 'Green'
        Say 'To reinstall it anyway, run:  $env:DATABUG_FORCE = "1"; irm https://raw.githubusercontent.com/hayes-roach/DataBug/main/install.ps1 | iex' 'Gray'
        return
    }
    if ($installed) {
        Say "Updating DataBug $($installed.Version) to $(if ($target) { $target } else { 'the latest version' })..."
    } else {
        Say "Installing DataBug $(if ($target) { $target } else { '' })..."
    }

    # --- download and verify -------------------------------------------------
    $dir = Join-Path $env:TEMP "DataBug-install-$PID"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $setup = Join-Path $dir 'DataBug-windows-x86_64-setup.exe'
    $sums = Join-Path $dir 'SHA256SUMS.txt'
    try {
        Invoke-WebRequest -Uri "$base/DataBug-windows-x86_64-setup.exe" -OutFile $setup -UseBasicParsing -TimeoutSec 600
        Invoke-WebRequest -Uri "$base/SHA256SUMS.txt" -OutFile $sums -UseBasicParsing -TimeoutSec 60
    } catch {
        Fail "Couldn't download DataBug: $($_.Exception.Message)"
        Fail "Check your internet connection, or download the installer from https://github.com/$repo/releases/latest"
        return
    }
    $want = (Get-Content $sums | Where-Object { $_ -match 'DataBug-windows-x86_64-setup\.exe' } | Select-Object -First 1) -replace '\s.*$', ''
    $got = (Get-FileHash -Algorithm SHA256 -Path $setup).Hash
    if (-not $want -or $got.ToLower() -ne $want.ToLower()) {
        Fail 'The download is damaged or was changed (its checksum does not match the release). Nothing was installed.'
        Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
        return
    }

    # --- close a running DataBug so its files can be replaced ----------------
    $running = Get-Process -Name 'DataBug' -ErrorAction SilentlyContinue
    if ($running) {
        Say 'Closing DataBug so it can be updated...'
        $running | ForEach-Object { [void]$_.CloseMainWindow() }
        $deadline = (Get-Date).AddSeconds(15)
        while ((Get-Process -Name 'DataBug' -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
        if (Get-Process -Name 'DataBug' -ErrorAction SilentlyContinue) {
            Fail 'DataBug is still open (it may be waiting on a dialog). Close it, then run this command again.'
            Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
            return
        }
    }

    # --- run the setup ---------------------------------------------------------
    $log = Join-Path $dir 'setup.log'
    $setupArgs = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CLOSEAPPLICATIONS', "/LOG=`"$log`"")
    $elevate = $false
    if ($installed -and $installed.AllUsers) {
        # update the existing all-users install in place (asks for admin rights)
        $setupArgs += '/ALLUSERS'
        $isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        $elevate = -not $isAdmin
        if ($elevate) { Say 'DataBug is installed for all users, so Windows will ask for admin rights.' }
    } else {
        $setupArgs += '/CURRENTUSER'
        if (-not $installed) { $setupArgs += '/TASKS=desktopicon' }
    }
    try {
        $p = if ($elevate) {
            Start-Process -FilePath $setup -ArgumentList $setupArgs -Verb RunAs -Wait -PassThru
        } else {
            Start-Process -FilePath $setup -ArgumentList $setupArgs -Wait -PassThru
        }
    } catch {
        Fail "The installer didn't start: $($_.Exception.Message)"
        if ($elevate) { Fail 'Admin rights are needed to update an install made for all users.' }
        return
    }

    if ($p.ExitCode -ne 0) {
        $why = switch ($p.ExitCode) {
            1 { 'the installer could not start.' }
            2 { 'the installation was cancelled before it started.' }
            3 { 'the installer hit an error while preparing to install.' }
            4 { 'the installer hit an error while installing.' }
            5 { 'the installation was stopped partway, usually because a DataBug file was in use (DataBug or one of its windows was still open) or could not be written.' }
            7 { 'Windows reported that the installation cannot continue right now (often a pending restart). Restart your PC and try again.' }
            8 { 'Windows needs a restart before DataBug can be installed. Restart your PC and try again.' }
            default { "the installer stopped with exit code $($p.ExitCode)." }
        }
        Fail "DataBug was not installed: $why"
        if (Test-Path $log) {
            Fail "Details from the installer log ($log):"
            Get-Content $log | Where-Object { $_ -match 'error|fail|denied|in use|abort|cannot' } | Select-Object -Last 8 | ForEach-Object { Write-Host "  $_" -ForegroundColor DarkGray }
            Fail 'Close DataBug if it is open and run the command again. If it still fails, please open an issue at https://github.com/hayes-roach/DataBug/issues and attach that log.'
        }
        return
    }
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue

    # --- start the new version ------------------------------------------------
    $exe = $null
    foreach ($k in $keys) {
        $entry = Get-ItemProperty -Path $k.Path -ErrorAction SilentlyContinue
        if ($entry -and $entry.InstallLocation) { $exe = Join-Path $entry.InstallLocation 'DataBug.exe'; break }
    }
    if (-not $exe) { $exe = Join-Path $env:LOCALAPPDATA 'Programs\DataBug\DataBug.exe' }
    if ($installed) {
        Say "DataBug updated$(if ($target) { " to $target" }). Your connections and settings are kept." 'Green'
    } else {
        Say 'DataBug installed. Find it in the Start menu.' 'Green'
    }
    if (Test-Path $exe) { Start-Process $exe }
}

Install-DataBug
