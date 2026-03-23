#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

ARCHITECTURE="${ARCHITECTURE:-$(uname -m)}"

case "$ARCHITECTURE" in
x86_64)
    TARGET_TRIPLE="x86_64-unknown-linux-gnu"
    ;;
arm64 | aarch64)
    if [[ "$(uname -s)" == "Darwin" ]]; then
        TARGET_TRIPLE="aarch64-apple-darwin"
    else
        TARGET_TRIPLE="aarch64-unknown-linux-gnu"
    fi
    ;;
*)
    echo "Unsupported architecture: $ARCHITECTURE" >&2
    exit 1
    ;;
esac

OUTPUT_PATH="$1"

# ---------------------------------------------------------------------------
# 1) Ensure target triple is installed
# ---------------------------------------------------------------------------
rustup target add "${TARGET_TRIPLE}" 2>/dev/null || true

# ---------------------------------------------------------------------------
# 2) Build the static lib
# ---------------------------------------------------------------------------
rm -f "target/${TARGET_TRIPLE}/release/libagno.a"

cargo build --release --lib --features "gpu" --target "${TARGET_TRIPLE}"

# ---------------------------------------------------------------------------
# 3) Copy the static library to the output path
# ---------------------------------------------------------------------------
cp "${REPO_ROOT}/target/${TARGET_TRIPLE}/release/libagno.a" "${OUTPUT_PATH}"

echo "--- Done: ${OUTPUT_PATH} ($(du -h "${OUTPUT_PATH}" | cut -f1)) ---"
