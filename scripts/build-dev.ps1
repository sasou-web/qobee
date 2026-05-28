# Build a "dev" copy of the Qobee desktop binary.
#
# Usage:
#     .\scripts\build-dev.ps1
#
# Produces `target\release\qobee-app-dev.exe`. The `-dev` suffix
# distinguishes this hand-built binary from the one produced by the
# bundle pipeline (`tauri build` / installers), which keeps the
# unsuffixed `qobee-app.exe` name for itself.
#
# Internally we use `tauri build --no-bundle` so the binary picks up
# the `frontendDist` config from `tauri.conf.json` (a plain
# `cargo build --release` skips the Tauri pipeline and the resulting
# exe falls back to `devUrl` -> `localhost:1420`, which fails to load
# at runtime).
#
# Optional flags:
#   -SkipUiBuild   Skip `npm run build` in `ui/`. Use when the dist
#                  bundle is already up to date and you only changed
#                  Rust code.

[CmdletBinding()]
param(
    [switch]$SkipUiBuild
)

$ErrorActionPreference = 'Stop'

# Resolve the workspace root from the script location so the call
# site can be anywhere.
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $root

# 1. Build the UI (vite). Tauri's bundling pipeline normally runs
#    `beforeBuildCommand` itself, but that command is `npm --prefix
#    ../ui run build`, which fails when invoked from inside
#    `src-tauri\` on Windows because the prefix resolution does not
#    handle the relative path the way npm expects. We run the UI
#    build ourselves and then point tauri at a config override that
#    disables the hook (`tauri.conf.dev-override.json` already sets
#    `beforeBuildCommand` and `beforeDevCommand` to empty strings).
if (-not $SkipUiBuild) {
    Write-Host '== Building UI (vite) ==' -ForegroundColor Cyan
    Push-Location (Join-Path $root 'ui')
    try {
        npm run build
        if ($LASTEXITCODE -ne 0) { throw "ui build failed (exit $LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
} else {
    Write-Host '== Skipping UI build (-SkipUiBuild) ==' -ForegroundColor DarkGray
}

# 2. Tauri release build without bundles. Run from src-tauri so the
#    Tauri CLI finds `tauri.conf.json` next to it. We use the local
#    binary stub installed by `npm install` in `ui/node_modules` so
#    we do not depend on a globally installed `tauri` CLI.
$tauriBin = Join-Path $root 'ui\node_modules\.bin\tauri.cmd'
if (-not (Test-Path $tauriBin)) {
    throw "tauri CLI not found at $tauriBin. Run 'npm install' inside the ui/ folder first."
}

Write-Host '== Building qobee-app (tauri build --no-bundle) ==' -ForegroundColor Cyan
Push-Location (Join-Path $root 'src-tauri')
try {
    & $tauriBin build --no-bundle --config tauri.conf.dev-override.json
    if ($LASTEXITCODE -ne 0) { throw "tauri build failed (exit $LASTEXITCODE)" }
} finally {
    Pop-Location
}

# 3. Rename the freshly built binary so it does not get confused
#    with the bundle output. Stop any running instance first so the
#    file is not locked by Windows.
$src = Join-Path $root 'target\release\qobee-app.exe'
$dst = Join-Path $root 'target\release\qobee-app-dev.exe'

if (-not (Test-Path $src)) {
    throw "expected binary not found: $src"
}

# If a prior `qobee-app-dev.exe` is still running, Windows holds a
# lock on it and Move-Item fails. Stop it best-effort before renaming.
Get-Process -Name 'qobee-app-dev' -ErrorAction SilentlyContinue | ForEach-Object {
    Write-Host "  stopping running $($_.Name) (pid $($_.Id))..."
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
}
# Also catch the case where the previous build was renamed but the
# process kept the original `qobee-app` name.
Get-Process -Name 'qobee-app' -ErrorAction SilentlyContinue | ForEach-Object {
    Write-Host "  stopping running $($_.Name) (pid $($_.Id))..."
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
}
Start-Sleep -Seconds 1

if (Test-Path $dst) { Remove-Item $dst -Force }
Move-Item $src $dst

Write-Host ''
Write-Host "Built: $dst" -ForegroundColor Green
$info = Get-Item $dst
Write-Host ("       {0:N2} MB, {1}" -f ($info.Length / 1MB), $info.LastWriteTime)
