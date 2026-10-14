#!/usr/bin/env powershell

[CmdletBinding()]
Param (
    [Switch]$SKIP_BUILD = $False,
    [String]$INSTALL_ROOT = "$env:LOCALAPPDATA\Programs\WarpSpeed"
)

$ErrorActionPreference = 'Stop'

# When this run began. Used below to leave alone anything a concurrent run
# staged after this point.
$RUN_STARTED_AT = Get-Date

# How long to wait for another install to finish before giving up. A release
# build takes around twenty minutes, so this has to be generous enough to queue
# behind one rather than fail underneath it.
$LOCK_TIMEOUT_MINUTES = 60

$WORKSPACE_ROOT = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$STAGING_TARGET_DIR = Join-Path $WORKSPACE_ROOT 'target\warpspeed-merge'
$STAGING_RELEASE_DIR = Join-Path $STAGING_TARGET_DIR 'release'

# Only one install at a time.
#
# Two runs a second apart will destroy each other: the first to finish prunes
# every version directory it does not recognise, and a directory the second run
# is still copying into is exactly that. The second run then fails partway
# through -- or worse, succeeds into a directory that has been deleted out from
# under it and leaves a half-populated install as `current`.
#
# The lock is an exclusive handle on a file rather than a mutex so that it is
# visible on disk while held, and Windows releases it when the process exits,
# including when it is killed. Held for the whole run, build included: cargo
# serializes concurrent builds on its own lock anyway, so a second run would
# queue there regardless, and this way the wait is explained rather than
# mysterious.
New-Item -ItemType Directory -Force -Path $INSTALL_ROOT | Out-Null
$lock_path = Join-Path $INSTALL_ROOT 'install.lock'
$lock_deadline = $RUN_STARTED_AT.AddMinutes($LOCK_TIMEOUT_MINUTES)
$lock_stream = $null
$announced_wait = $false
while (-not $lock_stream) {
    try {
        $lock_stream = [IO.File]::Open($lock_path, 'OpenOrCreate', 'ReadWrite', 'None')
    } catch [IO.IOException] {
        if ((Get-Date) -gt $lock_deadline) {
            throw "Timed out after $LOCK_TIMEOUT_MINUTES minutes waiting for another WarpSpeed install to finish. If no install is running, delete $lock_path and retry."
        }
        if (-not $announced_wait) {
            Write-Host 'Another WarpSpeed install is running; waiting for it to finish...'
            $announced_wait = $true
        }
        Start-Sleep -Seconds 5
    }
}
# $lock_stream stays in scope for the rest of the script; the handle is released
# when this process exits, however it exits.

# Every WarpSpeed process currently running, and the ones that would not say.
#
# This installer never terminates a running WarpSpeed: an instance is somebody's
# live terminal sessions. It only has to avoid deleting the files one is running
# from, which means knowing every version directory that is in use.
#
# `Get-Process warpspeed | ForEach-Object { $_.Path }` is not enough to know
# that. Reading `Path` opens the process, so an instance running at a higher
# integrity level than this script answers a non-elevated caller with an empty
# value rather than an error -- and this fork's `AppId` varies with elevation
# precisely so an elevated WarpSpeed can run alongside a normal one, which makes
# that the expected case rather than an exotic one. `Win32_Process` avoids the
# handle for most processes, so it is tried first, but it reports an empty
# `ExecutablePath` for the same reason often enough that the caller still has to
# handle not knowing.
#
# `UnidentifiedProcessIds` is therefore part of the answer, not an error: a
# caller that cannot rule a version out must leave every version alone.
function Get-RunningWarpSpeedInstances {
    $executable_paths = New-Object Collections.Generic.List[string]
    $unidentified_process_ids = New-Object Collections.Generic.List[int]

    $processes = @(
        Get-CimInstance Win32_Process -Filter "Name='warpspeed.exe' OR Name='warp-oss.exe'" -ErrorAction SilentlyContinue
    )
    foreach ($process in $processes) {
        $path = $process.ExecutablePath
        if (-not $path) {
            $path = try {
                (Get-Process -Id $process.ProcessId -ErrorAction Stop).Path
            } catch {
                $null
            }
        }
        if ($path) {
            $executable_paths.Add($path)
        } else {
            $unidentified_process_ids.Add([int]$process.ProcessId)
        }
    }

    [PSCustomObject]@{
        ExecutablePaths        = $executable_paths.ToArray()
        UnidentifiedProcessIds = $unidentified_process_ids.ToArray()
    }
}

# What is about to be compiled, read before compiling it.
#
# A release build takes a quarter of an hour, and asking git afterwards describes the tree as it
# is when the build finishes rather than the one it consumed. Any commit, checkout or stray edit
# in between -- and a quarter of an hour is long enough for all three -- lands under a version
# label naming a revision the binary was never built from, or marked `-dirty` for changes that
# were not on disk when a single file was compiled. A version directory is the only record of
# what a build contains, and one that describes the wrong tree is worse than no record at all,
# because the release notes below are generated from it.
#
# The label is still a claim about the start of the build rather than a proof: cargo reads its
# sources as it goes, so an edit made mid-build can reach the binary without reaching the label.
# Nothing short of building from a clean checkout can close that, and the case worth ruling out
# is the ordinary one -- committing while a build runs.
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

if (-not $SKIP_BUILD) {
    $previous_target_dir = $env:CARGO_TARGET_DIR
    $previous_full_profile = $env:CARGO_FULL_PROFILE
    $previous_protoc = $env:PROTOC
    $previous_app_name = $env:WARP_APP_NAME

    try {
        $env:CARGO_TARGET_DIR = $STAGING_TARGET_DIR
        $env:CARGO_FULL_PROFILE = 'warpspeed-merge/release'
        $env:WARP_APP_NAME = 'WarpSpeed by STOZN'
        if (-not $env:PROTOC) {
            $protoc = Get-Command protoc -ErrorAction Stop
            $env:PROTOC = $protoc.Source
        }

        Push-Location $WORKSPACE_ROOT
        try {
            cargo build -p warp --release --bin warpspeed --features tui,crash_reporting
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
        $env:WARP_APP_NAME = $previous_app_name
    }
}

$runtime_files = @(
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
start "" /D "%USERPROFILE%" "%~dp0versions\%WARPSPEED_VERSION%\warpspeed.exe" %*
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
# Release notes for this version: the commits between the build being replaced
# and this one. WarpSpeed reads this file from beside its executable and shows
# it in the Resource Center's what's-new section, because the changelog server
# only knows about tagged releases and has nothing to say about a local build.
#
# The version directory name is <yyyyMMdd>-<HHmmssfff>-<revision>[-dirty], so the
# revision is the third dash-separated component.
$release_notes_path = Join-Path $version_dir 'release-notes.md'
$previous_revision = if ($previous_version) {
    $parts = $previous_version.Split('-')
    if ($parts.Count -ge 3) { $parts[2] } else { $null }
} else {
    $null
}

$release_notes = if (-not $previous_revision) {
    "First install of WarpSpeed from this repository, at ``$revision``."
} else {
    Push-Location $WORKSPACE_ROOT
    try {
        # Verify the revision is still in the repository before asking for a
        # range against it: a rebase or a reset can leave the previous install
        # pointing at a commit that no longer exists, and `git log` would fail.
        git cat-file -e "$previous_revision^{commit}" 2>$null
        if ($LASTEXITCODE -ne 0) {
            "Replaces ``$previous_revision``, which is no longer in the repository, so the list of changes cannot be determined."
        } else {
            $log = git log --pretty=format:'- %s (%h)' "$previous_revision..HEAD"
            if ($LASTEXITCODE -ne 0) {
                "Could not determine the changes since ``$previous_revision``."
            } elseif (-not $log) {
                "No new commits since ``$previous_revision``. This build differs only by uncommitted changes."
            } else {
                ($log -join "`n")
            }
        }
    } finally {
        Pop-Location
    }
}

$dirty_note = if ($dirty) {
    "`n`nBuilt from a tree with uncommitted changes, so it also contains work that is not in any commit above."
} else {
    ''
}
[IO.File]::WriteAllText(
    $release_notes_path,
    "## $version`n`n$release_notes$dirty_note`n",
    [Text.Encoding]::UTF8
)

$current_temp_path = Join-Path $INSTALL_ROOT 'current.txt.new'
[IO.File]::WriteAllText($current_temp_path, $version, [Text.Encoding]::ASCII)
Move-Item -Force -LiteralPath $current_temp_path -Destination $current_path
Copy-Item -LiteralPath (Join-Path $version_dir 'icon.ico') -Destination $INSTALL_ROOT -Force

# Register the stable dashboard-to-WarpSpeed handoff. The dashboard sends only
# environment, fleet and controller job identifiers; WarpSpeed validates all of
# them again before opening its fixed managed-session viewer command.
$protocol_root = 'HKCU:\Software\Classes\warpspeed'
New-Item -Path $protocol_root -Force | Out-Null
Set-Item -Path $protocol_root -Value 'URL:WarpSpeed by STOZN managed session'
New-ItemProperty -Path $protocol_root -Name 'URL Protocol' -Value '' -PropertyType String -Force | Out-Null
$protocol_icon = Join-Path $protocol_root 'DefaultIcon'
New-Item -Path $protocol_icon -Force | Out-Null
Set-Item -Path $protocol_icon -Value "$(Join-Path $version_dir 'warpspeed.exe'),0"
$protocol_command = Join-Path $protocol_root 'shell\open\command'
New-Item -Path $protocol_command -Force | Out-Null
Set-Item -Path $protocol_command -Value "`"$(Join-Path $version_dir 'warpspeed.exe')`" `"%1`""

$start_menu_dir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$wsh = New-Object -ComObject WScript.Shell
$shortcuts = @{
    'WarpSpeed by STOZN.lnk' = $launcher_path
    'WarpSpeed by STOZN [a].lnk' = $profile_launchers['a']
    'WarpSpeed by STOZN [b].lnk' = $profile_launchers['b']
    'WarpSpeed by STOZN [c].lnk' = $profile_launchers['c']
}
@('WarpSpeed.lnk', 'WarpSpeed [a].lnk', 'WarpSpeed [b].lnk', 'WarpSpeed [c].lnk') | ForEach-Object {
    Remove-Item -LiteralPath (Join-Path $start_menu_dir $_) -Force -ErrorAction SilentlyContinue
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
setlocal
set "WARPSPEED_INSTALL_ROOT=$INSTALL_ROOT\"
set /p WARPSPEED_VERSION=<"$INSTALL_ROOT\current.txt"
if not defined WARPSPEED_VERSION exit /b 1
"$INSTALL_ROOT\versions\%WARPSPEED_VERSION%\warpspeed.exe" %*
exit /b %ERRORLEVEL%
"@
[IO.File]::WriteAllText($cli_shim_path, $cli_shim, [Text.Encoding]::ASCII)

$protected_versions = @($version, $previous_version) | Where-Object { $_ }
$running = Get-RunningWarpSpeedInstances
$running_executables = $running.ExecutablePaths

# One unidentifiable instance makes every version suspect, and the only safe
# amount of pruning is then none. Stale directories are cheap; the alternative
# is deleting conpty.dll, the icon and the release notes out from under a live
# window, which is what "the installer killed my session" looks like from the
# outside.
if ($running.UnidentifiedProcessIds.Count -gt 0) {
    $unidentified = $running.UnidentifiedProcessIds -join ', '
    Write-Warning "Skipping cleanup of old WarpSpeed versions: $($running.UnidentifiedProcessIds.Count) running instance(s) (PID $unidentified) would not report an executable path, so which versions are in use cannot be determined. Re-run this installer elevated, or with those instances closed, to clean up."
} else {
    Get-ChildItem -LiteralPath $versions_dir -Directory | ForEach-Object {
        $candidate = $_
        $candidate_prefix = $candidate.FullName.TrimEnd('\') + '\'
        $is_running = $running_executables | Where-Object {
            $_.StartsWith($candidate_prefix, [StringComparison]::OrdinalIgnoreCase)
        }
        # Anything that appeared after this run started belongs to someone else --
        # an install running an older copy of this script, without the lock above.
        # Deleting it is what broke the run that staged it. Second line of defence:
        # with the lock held, this should never be true.
        $staged_after_this_run = $candidate.CreationTime -ge $RUN_STARTED_AT
        if ($staged_after_this_run) {
            Write-Warning "Leaving WarpSpeed version $($candidate.Name) alone: it was staged after this install started, so another install is using it."
        }
        if ($protected_versions -notcontains $candidate.Name -and
            -not $is_running -and
            -not $staged_after_this_run) {
            try {
                Remove-Item -LiteralPath $candidate.FullName -Recurse -Force
            } catch {
                Write-Warning "Could not remove old WarpSpeed version $($candidate.Name): $_"
            }
        }
    }

    Get-ChildItem -LiteralPath $versions_dir -Filter 'warp-oss.exe' -File -Recurse |
        ForEach-Object {
            $obsolete_executable = $_
            $in_use = $running_executables | Where-Object {
                $_.Equals($obsolete_executable.FullName, [StringComparison]::OrdinalIgnoreCase)
            }
            if (-not $in_use) {
                try {
                    Remove-Item -LiteralPath $obsolete_executable.FullName -Force
                } catch {
                    Write-Warning "Could not remove obsolete Warp OSS executable $($obsolete_executable.FullName): $_"
                }
            }
        }
}

$running_count = $running_executables.Count + $running.UnidentifiedProcessIds.Count
if ($running_count -gt 0) {
    Write-Host "$running_count WarpSpeed process(es) are still running and were left alone. Each keeps serving its sessions from the version it launched with; start WarpSpeed from the Start Menu to use $version. Each version runs its own terminal supervisor, so 'Restart window in new process' keeps a window on the version it already runs."
}

Write-Host "Installed WarpSpeed $version"
Write-Host "Executable: $(Join-Path $version_dir 'warpspeed.exe')"
Write-Host "Launcher:   $launcher_path"
