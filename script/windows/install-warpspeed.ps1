#!/usr/bin/env powershell

[CmdletBinding()]
Param (
    [Switch]$SKIP_BUILD = $False,
    [String]$INSTALL_ROOT = "$env:LOCALAPPDATA\Programs\WarpSpeed"
)

$ErrorActionPreference = 'Stop'

$WORKSPACE_ROOT = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$STAGING_TARGET_DIR = Join-Path $WORKSPACE_ROOT 'target\warpspeed-merge'
$STAGING_RELEASE_DIR = Join-Path $STAGING_TARGET_DIR 'release'

if (-not $SKIP_BUILD) {
    $previous_target_dir = $env:CARGO_TARGET_DIR
    $previous_full_profile = $env:CARGO_FULL_PROFILE
    $previous_protoc = $env:PROTOC

    try {
        $env:CARGO_TARGET_DIR = $STAGING_TARGET_DIR
        $env:CARGO_FULL_PROFILE = 'warpspeed-merge/release'
        if (-not $env:PROTOC) {
            $protoc = Get-Command protoc -ErrorAction Stop
            $env:PROTOC = $protoc.Source
        }

        Push-Location $WORKSPACE_ROOT
        try {
            cargo build -p warp --release --bin warp-oss --bin warpspeed
            if ($LASTEXITCODE -ne 0) {
                throw "Failed to build warpspeed (exit code $LASTEXITCODE)"
            }
        } finally {
            Pop-Location
        }
    } finally {
        $env:CARGO_TARGET_DIR = $previous_target_dir
        $env:CARGO_FULL_PROFILE = $previous_full_profile
        $env:PROTOC = $previous_protoc
    }
}

$runtime_files = @(
    'warp-oss.exe',
    'warpspeed.exe',
    'conpty.dll',
    'dxcompiler.dll',
    'dxil.dll',
    'icon.ico'
)
foreach ($file in $runtime_files) {
    $path = Join-Path $STAGING_RELEASE_DIR $file
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Missing staged WarpSpeed runtime file: $path"
    }
}

$native_architecture = if ($env:PROCESSOR_ARCHITEW6432) {
    $env:PROCESSOR_ARCHITEW6432
} else {
    $env:PROCESSOR_ARCHITECTURE
}
$runtime_architecture = switch ($native_architecture) {
    'AMD64' { 'x64' }
    'ARM64' { 'arm64' }
    default { throw "Unsupported Windows architecture: $native_architecture" }
}
$open_console = Join-Path $STAGING_RELEASE_DIR "$runtime_architecture\OpenConsole.exe"
if (-not (Test-Path -LiteralPath $open_console -PathType Leaf)) {
    throw "Missing staged WarpSpeed runtime file: $open_console"
}

Push-Location $WORKSPACE_ROOT
try {
    $revision = (git rev-parse --short=12 HEAD).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw 'Failed to determine the current git revision'
    }
    $dirty = if (git status --porcelain) { '-dirty' } else { '' }
} finally {
    Pop-Location
}

$timestamp = [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmssfff')
$version = "$timestamp-$revision$dirty"
$versions_dir = Join-Path $INSTALL_ROOT 'versions'
$version_dir = Join-Path $versions_dir $version
New-Item -ItemType Directory -Force -Path (Join-Path $version_dir $runtime_architecture) |
    Out-Null

foreach ($file in $runtime_files) {
    Copy-Item -LiteralPath (Join-Path $STAGING_RELEASE_DIR $file) -Destination $version_dir
}
Copy-Item -LiteralPath $open_console -Destination (Join-Path $version_dir $runtime_architecture)

$launcher_path = Join-Path $INSTALL_ROOT 'launch-warpspeed.cmd'
$launcher = @'
@echo off
setlocal
set "WARPSPEED_INSTALL_ROOT=%~dp0"
set /p WARPSPEED_VERSION=<"%~dp0current.txt"
if not defined WARPSPEED_VERSION exit /b 1
start "" "%~dp0versions\%WARPSPEED_VERSION%\warpspeed.exe" %*
'@
[IO.File]::WriteAllText($launcher_path, $launcher, [Text.Encoding]::ASCII)

$profile_launchers = @{}
foreach ($profile in @('a', 'b', 'c')) {
    $profile_launcher_path = Join-Path $INSTALL_ROOT "launch-warpspeed-$profile.cmd"
    $profile_launcher = @"
@echo off
set "WARPSPEED_PROFILE=$profile"
call "%~dp0launch-warpspeed.cmd" %*
"@
    [IO.File]::WriteAllText($profile_launcher_path, $profile_launcher, [Text.Encoding]::ASCII)
    $profile_launchers[$profile] = $profile_launcher_path
}

$current_path = Join-Path $INSTALL_ROOT 'current.txt'
$previous_version = if (Test-Path -LiteralPath $current_path) {
    (Get-Content -LiteralPath $current_path -Raw).Trim()
} else {
    $null
}
$current_temp_path = Join-Path $INSTALL_ROOT 'current.txt.new'
[IO.File]::WriteAllText($current_temp_path, $version, [Text.Encoding]::ASCII)
Move-Item -Force -LiteralPath $current_temp_path -Destination $current_path
Copy-Item -LiteralPath (Join-Path $version_dir 'icon.ico') -Destination $INSTALL_ROOT -Force

$start_menu_dir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$wsh = New-Object -ComObject WScript.Shell
$shortcuts = @{
    'WarpSpeed.lnk' = $launcher_path
    'WarpSpeed [a].lnk' = $profile_launchers['a']
    'WarpSpeed [b].lnk' = $profile_launchers['b']
    'WarpSpeed [c].lnk' = $profile_launchers['c']
}
foreach ($entry in $shortcuts.GetEnumerator()) {
    $shortcut_path = Join-Path $start_menu_dir $entry.Key
    $shortcut = $wsh.CreateShortcut($shortcut_path)
    $shortcut.TargetPath = "$env:SystemRoot\System32\cmd.exe"
    $shortcut.Arguments = "/d /c `"`"$($entry.Value)`"`""
    $shortcut.WorkingDirectory = $INSTALL_ROOT
    $shortcut.IconLocation = "$(Join-Path $INSTALL_ROOT 'icon.ico'),0"
    $shortcut.Save()
}

$local_bin = Join-Path $HOME '.local\bin'
New-Item -ItemType Directory -Force -Path $local_bin | Out-Null
$cli_shim_path = Join-Path $local_bin 'warpspeed.cmd'
$cli_shim = @"
@echo off
call "$launcher_path" %*
"@
[IO.File]::WriteAllText($cli_shim_path, $cli_shim, [Text.Encoding]::ASCII)

$protected_versions = @($version, $previous_version) | Where-Object { $_ }
$running_executables = @(
    Get-Process warpspeed -ErrorAction SilentlyContinue |
        ForEach-Object { $_.Path } |
        Where-Object { $_ }
)
Get-ChildItem -LiteralPath $versions_dir -Directory | ForEach-Object {
    $candidate = $_
    $candidate_prefix = $candidate.FullName.TrimEnd('\') + '\'
    $is_running = $running_executables | Where-Object {
        $_.StartsWith($candidate_prefix, [StringComparison]::OrdinalIgnoreCase)
    }
    if ($protected_versions -notcontains $candidate.Name -and -not $is_running) {
        try {
            Remove-Item -LiteralPath $candidate.FullName -Recurse -Force
        } catch {
            Write-Warning "Could not remove old WarpSpeed version $($candidate.Name): $_"
        }
    }
}

Write-Host "Installed WarpSpeed $version"
Write-Host "Executable: $(Join-Path $version_dir 'warpspeed.exe')"
Write-Host "Launcher:   $launcher_path"
