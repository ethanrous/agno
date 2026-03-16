#!/usr/bin/env bash
set -euo pipefail

# This script builds a fully static version of libagno by merging the main Rust static library with all of its transitive static dependencies (notably libheif and its codecs). The resulting archive can be linked into other projects without needing to worry about dynamic dependencies.

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
cargo build --release --features gpu --target "${TARGET_TRIPLE}"

# ---------------------------------------------------------------------------
# 3) Collect all libheif transitive static deps and merge into libagno.a
#
#    libheif links against codecs (libde265, x265, aom, etc.) whose symbols
#    must be present in the final archive. We parse pkg-config to discover
#    the full set of -l flags, locate the corresponding .a files, and merge
#    them all into the output archive alongside libagno.a.
# ---------------------------------------------------------------------------
TARGET_ARCHIVE="target/${TARGET_TRIPLE}/release/libagno.a"
MERGE_LIBS=("${TARGET_ARCHIVE}")

# Parse -l flags from pkg-config (skip system libs provided by the OS)
SYSTEM_LIBS="c++ stdc++ dl m pthread"

find_static_lib() {
    local name="$1"
    # Use pkg-config lib dirs as search roots
    local search_dirs
    search_dirs=$(pkg-config --libs-only-L libheif 2>/dev/null | tr ' ' '\n' | sed 's/^-L//')

    for dir in $search_dirs /opt/homebrew/lib /usr/local/lib /usr/lib; do
        [[ -f "$dir/lib${name}.a" ]] && echo "$dir/lib${name}.a" && return
    done

    # Homebrew Cellar fallback (static libs sometimes aren't symlinked)
    local result
    result=$(find /opt/homebrew/Cellar -name "lib${name}.a" -print -quit 2>/dev/null || true)
    [[ -n "$result" ]] && echo "$result" && return

    return 1
}

# Extract library names from pkg-config
LINK_FLAGS=$(pkg-config --libs --static libheif 2>/dev/null || true)

if [[ -n "$LINK_FLAGS" ]]; then
    for flag in $LINK_FLAGS; do
        # Only process -l flags
        [[ "$flag" != -l* ]] && continue
        libname="${flag#-l}"

        # Skip system libs
        skip=false
        for sys in $SYSTEM_LIBS; do
            [[ "$libname" == "$sys" ]] && skip=true && break
        done
        $skip && continue

        static_path=$(find_static_lib "$libname" || true)
        if [[ -n "$static_path" ]]; then
            MERGE_LIBS+=("$static_path")
            echo "  Found: lib${libname}.a -> ${static_path}"
        else
            echo "  Warning: lib${libname}.a not found (will need dynamic linking)" >&2
        fi
    done
else
    echo "Warning: pkg-config failed for libheif, archive may have unresolved symbols" >&2
fi

echo "--- Merging ${#MERGE_LIBS[@]} archives into ${OUTPUT_PATH} ---"

if [[ "$(uname -s)" == "Darwin" ]]; then
    libtool -static -o "${OUTPUT_PATH}" "${MERGE_LIBS[@]}"
else
    MERGE_TMP=$(mktemp -d)
    for lib in "${MERGE_LIBS[@]}"; do
        cd "$MERGE_TMP"
        ar x "$lib"
    done
    cd "$MERGE_TMP"
    ar rcs "${OUTPUT_PATH}" ./*.o
    rm -rf "$MERGE_TMP"
fi

echo "--- Done: ${OUTPUT_PATH} ($(du -h "${OUTPUT_PATH}" | cut -f1)) ---"
