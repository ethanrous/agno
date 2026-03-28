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
AGNO_FEATURES="${AGNO_FEATURES:-gpu,jpeg,png,webp,pdf}"
LIB_DIR="${REPO_ROOT}/lib"

if [[ "$AGNO_FEATURES" == *"pdf-pdfium"* ]]; then
    if [[ ! -e "${LIB_DIR}/libpdfium.a" ]]; then
        echo "--- Downloading libpdfium.a ---"

        # Pick the right asset from kernoeb/pdfium-static
        case "$TARGET_TRIPLE" in
        x86_64-unknown-linux-gnu)  PDFIUM_ASSET="libpdfium-linux-x64.a" ;;
        aarch64-apple-darwin)      PDFIUM_ASSET="libpdfium-macos-arm64.a" ;;
        *)
            echo "No static pdfium binary for ${TARGET_TRIPLE}, disabling pdf-pdfium" >&2
            AGNO_FEATURES="${AGNO_FEATURES//,pdf-pdfium/}"
            AGNO_FEATURES="${AGNO_FEATURES//pdf-pdfium,/}"
            AGNO_FEATURES="${AGNO_FEATURES//pdf-pdfium/}"
            ;;
        esac
    fi

    # Download if we still need pdf-pdfium and don't have it yet
    if [[ "$AGNO_FEATURES" == *"pdf-pdfium"* && ! -e "${LIB_DIR}/libpdfium.a" ]]; then
        latest_release=$(gh api -H "Accept: application/vnd.github+json" \
            '/repos/kernoeb/pdfium-static/releases?per_page=1' | jq -r '.[0].tag_name')
        mkdir -p "${LIB_DIR}"
        curl -fsSL "https://github.com/kernoeb/pdfium-static/releases/download/${latest_release}/${PDFIUM_ASSET}" \
            -o "${LIB_DIR}/libpdfium.a"
    fi

    if [[ "$AGNO_FEATURES" == *"pdf-pdfium"* ]]; then
        export PDFIUM_STATIC_LIB_PATH="${LIB_DIR}"
    fi
fi

# ---------------------------------------------------------------------------
# 3) Build the static lib
# ---------------------------------------------------------------------------
rm -f "target/${TARGET_TRIPLE}/release/libagno.a"

cargo build --release --lib --no-default-features --features "${AGNO_FEATURES}" --target "${TARGET_TRIPLE}"

# ---------------------------------------------------------------------------
# 4) Copy the static library to the output path
# ---------------------------------------------------------------------------
cp "${REPO_ROOT}/target/${TARGET_TRIPLE}/release/libagno.a" "${OUTPUT_PATH}"

echo "--- Done: ${OUTPUT_PATH} ($(du -h "${OUTPUT_PATH}" | cut -f1)) ---"
