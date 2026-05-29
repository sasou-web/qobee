# Point git at the repo's committed hooks (.githooks/) so the
# pre-push gate (rustfmt + clippy) runs before every push.
#
# Run once per clone from the workspace root:
#     ./scripts/setup-hooks.ps1
#
# This only sets a local git config value; it makes no network calls
# and changes no tracked files.

$ErrorActionPreference = "Stop"

# Resolve the repo root (parent of this script's directory) so the
# command works regardless of the current working directory.
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    git config core.hooksPath .githooks
    Write-Host "core.hooksPath set to .githooks"
    Write-Host "pre-push will now run 'cargo fmt --check' + 'cargo clippy -D warnings'."
    Write-Host "Bypass a single push with 'git push --no-verify'."
} finally {
    Pop-Location
}
