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
    # Determine the correct pdfium asset for this target (or disable the feature)
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

if [[ "$AGNO_FEATURES" == *"pdf-pdfium"* ]]; then
    # Always re-download to avoid architecture mismatches from cached builds
    echo "--- Downloading libpdfium.a (${PDFIUM_ASSET}) ---"
    latest_release=$(gh api -H "Accept: application/vnd.github+json" \
        '/repos/kernoeb/pdfium-static/releases?per_page=1' | jq -r '.[0].tag_name')
    mkdir -p "${LIB_DIR}"
    curl -fsSL "https://github.com/kernoeb/pdfium-static/releases/download/${latest_release}/${PDFIUM_ASSET}" \
        -o "${LIB_DIR}/libpdfium.a"
    export PDFIUM_STATIC_LIB_PATH="${LIB_DIR}"
fi

# ---------------------------------------------------------------------------
# 3) Build the static lib
# ---------------------------------------------------------------------------
rm -f "target/${TARGET_TRIPLE}/release/libagno.a"

cargo build --release --lib --no-default-features --features "${AGNO_FEATURES}" --target "${TARGET_TRIPLE}"

# ---------------------------------------------------------------------------
# 4) Merge pdfium into libagno.a so consumers have zero extra dependencies
# ---------------------------------------------------------------------------
AGNO_LIB="${REPO_ROOT}/target/${TARGET_TRIPLE}/release/libagno.a"

if [[ "$AGNO_FEATURES" == *"pdf-pdfium"* && -e "${LIB_DIR}/libpdfium.a" ]]; then
    echo "--- Merging libpdfium.a into libagno.a ---"
    MERGED="${REPO_ROOT}/target/${TARGET_TRIPLE}/release/libagno-merged.a"

    if [[ "$(uname -s)" == "Darwin" ]]; then
        libtool -static -o "${MERGED}" "${AGNO_LIB}" "${LIB_DIR}/libpdfium.a"
    else
        # Use ar MRI script to combine archives
        ARBIN="ar"
        if [[ "$TARGET_TRIPLE" == "aarch64-unknown-linux-gnu" ]]; then
            ARBIN="aarch64-linux-gnu-ar"
        fi
        cat > /tmp/merge-ar.mri <<MRIEOF
CREATE ${MERGED}
ADDLIB ${AGNO_LIB}
ADDLIB ${LIB_DIR}/libpdfium.a
SAVE
END
MRIEOF
        "${ARBIN}" -M < /tmp/merge-ar.mri
        rm -f /tmp/merge-ar.mri
    fi

    mv "${MERGED}" "${AGNO_LIB}"
fi

# ---------------------------------------------------------------------------
# 5) Copy the static library to the output path
# ---------------------------------------------------------------------------
cp "${AGNO_LIB}" "${OUTPUT_PATH}"

echo "--- Done: ${OUTPUT_PATH} ($(du -h "${OUTPUT_PATH}" | cut -f1)) ---"
