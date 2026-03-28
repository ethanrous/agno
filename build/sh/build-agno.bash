#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# TARGET_TRIPLE can be set directly; otherwise derived from host architecture.
if [[ -z "${TARGET_TRIPLE:-}" ]]; then
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
fi

OUTPUT_PATH="$1"

# ---------------------------------------------------------------------------
# 1) Ensure target triple is installed
# ---------------------------------------------------------------------------
rustup target add "${TARGET_TRIPLE}" 2>/dev/null || true

# ---------------------------------------------------------------------------
# 2) Download pdfium static lib if pdf-pdfium feature is requested
# ---------------------------------------------------------------------------
AGNO_FEATURES="${AGNO_FEATURES:-gpu}"
LIB_DIR="${REPO_ROOT}/lib"

if [[ "$AGNO_FEATURES" == *"pdf-pdfium"* ]]; then
    if [[ ! -e "${LIB_DIR}/libpdfium.a" ]]; then
        echo "--- Downloading libpdfium.a ---"

        # Pick the right platform archive
        case "$TARGET_TRIPLE" in
        x86_64-unknown-linux-gnu)   PDFIUM_ARCHIVE="linux-x64.tgz" ;;
        aarch64-unknown-linux-gnu)  PDFIUM_ARCHIVE="linux-arm64.tgz" ;;
        aarch64-apple-darwin)       PDFIUM_ARCHIVE="mac-arm64.tgz" ;;
        x86_64-apple-darwin)        PDFIUM_ARCHIVE="mac-x64.tgz" ;;
        *)
            echo "No pdfium binary available for ${TARGET_TRIPLE}" >&2
            exit 1
            ;;
        esac

        latest_release=$(gh api -H "Accept: application/vnd.github+json" \
            '/repos/nicholasgasior/pdfium-binaries/releases?per_page=1' | jq -r '.[0].tag_name')
        curl -fsSL "https://github.com/nicholasgasior/pdfium-binaries/releases/download/${latest_release}/${PDFIUM_ARCHIVE}" \
            -o /tmp/pdfium.tgz
        tar -xzf /tmp/pdfium.tgz -C /tmp/pdfium-extract
        mkdir -p "${LIB_DIR}"
        find /tmp/pdfium-extract -name "libpdfium.a" -exec mv {} "${LIB_DIR}/libpdfium.a" \;
        rm -rf /tmp/pdfium.tgz /tmp/pdfium-extract
    fi

    export PDFIUM_STATIC_LIB_PATH="${LIB_DIR}"
fi

# ---------------------------------------------------------------------------
# 3) Build the static lib
# ---------------------------------------------------------------------------
rm -f "target/${TARGET_TRIPLE}/release/libagno.a"

cargo build --release --lib --features "${AGNO_FEATURES}" --target "${TARGET_TRIPLE}"

# ---------------------------------------------------------------------------
# 4) Copy the static library to the output path
# ---------------------------------------------------------------------------
cp "${REPO_ROOT}/target/${TARGET_TRIPLE}/release/libagno.a" "${OUTPUT_PATH}"

echo "--- Done: ${OUTPUT_PATH} ($(du -h "${OUTPUT_PATH}" | cut -f1)) ---"
