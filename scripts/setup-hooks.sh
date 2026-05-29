#!/bin/sh
# Point git at the repo's committed hooks (.githooks/) so the
# pre-push gate (rustfmt + clippy) runs before every push.
#
# Run once per clone from the workspace root:
#     ./scripts/setup-hooks.sh
#
# This only sets a local git config value; it makes no network calls
# and changes no tracked files.
set -e

# Resolve the repo root (parent of this script's directory).
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(dirname -- "$SCRIPT_DIR")
cd "$REPO_ROOT"

git config core.hooksPath .githooks
# Make sure the hook is executable on Unix clones.
chmod +x .githooks/pre-push 2>/dev/null || true

echo "core.hooksPath set to .githooks"
echo "pre-push will now run 'cargo fmt --check' + 'cargo clippy -D warnings'."
echo "Bypass a single push with 'git push --no-verify'."
