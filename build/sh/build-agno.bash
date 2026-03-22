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
# Select HEIC backend: "libheif" (default, production) or "native" (experimental)
HEIC_BACKEND="${HEIC_BACKEND:-libheif}"
EXTRA_FEATURES=""
if [ "$HEIC_BACKEND" = "native" ]; then
    EXTRA_FEATURES=",heic-experimental-decoder"
    echo "Using experimental native HEVC decoder for HEIC"
fi

# Build libheif and libde265 from source BEFORE cargo build: libheif-sys requires
# libheif >= 1.21 via pkg-config at cargo build time, so it must be present first.
# apt only ships ~1.17.x; we build static libs from source for later merging too.
if [ "$HEIC_BACKEND" = "libheif" ] && [[ "$(uname -s)" != "Darwin" ]]; then
    ./build/sh/transient/build-libheif.bash
fi

cargo build --release --lib --features "gpu${EXTRA_FEATURES}" --target "${TARGET_TRIPLE}"

# ---------------------------------------------------------------------------
# 3) Merge C library dependencies into the static archive
# ---------------------------------------------------------------------------
RUST_LIB="${REPO_ROOT}/target/${TARGET_TRIPLE}/release/libagno.a"

if [ "$HEIC_BACKEND" = "libheif" ]; then
    echo "Merging libheif and all transitive C deps into static archive..."
    MERGE_DIR=$(mktemp -d)
    trap "rm -rf ${MERGE_DIR}" EXIT

    # Discover all -l flags from pkg-config, resolve each to a static .a file.
    # libheif depends on: libde265, x265, aom, libvmaf, sharpyuv (+ system libs).
    STATIC_LIBS=("$RUST_LIB")
    MISSING=()

    # Map pkg-config -l names to their pkg-config package names for --variable=libdir
    declare -A LIB_TO_PKG=(
        [heif]=libheif [de265]=libde265 [x265]=x265
        [aom]=aom [vmaf]=libvmaf [sharpyuv]=libsharpyuv
    )

    for lib_name in "${!LIB_TO_PKG[@]}"; do
        pkg="${LIB_TO_PKG[$lib_name]}"
        lib_dir=$(pkg-config --variable=libdir "$pkg" 2>/dev/null || true)
        lib_path="${lib_dir}/lib${lib_name}.a"
        if [ -n "$lib_dir" ] && [ -f "$lib_path" ]; then
            STATIC_LIBS+=("$lib_path")
            echo "  Found: $lib_path"
        else
            MISSING+=("lib${lib_name}.a (pkg: ${pkg})")
        fi
    done

    if [ ${#MISSING[@]} -gt 0 ]; then
        echo "WARNING: Some static libraries not found (may cause link errors):" >&2
        for m in "${MISSING[@]}"; do echo "  - $m" >&2; done
    fi

    if [[ "$(uname -s)" == "Darwin" ]]; then
        libtool -static -o "${MERGE_DIR}/combined.a" "${STATIC_LIBS[@]}"
    else
        for lib in "${STATIC_LIBS[@]}"; do
            SUBDIR="${MERGE_DIR}/$(basename "$lib" .a)_$$"
            mkdir -p "$SUBDIR"
            (cd "$SUBDIR" && ar x "$lib")
        done
        ar rcs "${MERGE_DIR}/combined.a" "${MERGE_DIR}"/*/*.o
    fi

    cp "${MERGE_DIR}/combined.a" "${OUTPUT_PATH}"
else
    cp "$RUST_LIB" "${OUTPUT_PATH}"
fi

echo "--- Done: ${OUTPUT_PATH} ($(du -h "${OUTPUT_PATH}" | cut -f1)) ---"
