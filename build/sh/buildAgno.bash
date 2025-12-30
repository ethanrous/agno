#!/usr/bin/env bash
set -euo pipefail

ARCHITECTURE="${ARCHITECTURE:-$(uname -m)}"

case "$ARCHITECTURE" in
x86_64)
    MUSL_PREFIX="x86_64"

    # check if this is ubuntu or alpine
    if [[ -f /etc/os-release ]]; then
        . /etc/os-release
        if [[ "$ID" == "alpine" ]]; then
            TARGET_TRIPLE="x86_64-unknown-linux-musl"
            CARGO_LINKER_ENV_VAR="CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER"
        else
            TARGET_TRIPLE="x86_64-unknown-linux-gnu"
            CARGO_LINKER_ENV_VAR="CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER"
        fi
    fi
    ;;
arm64)
    MUSL_PREFIX="aarch64"

    if [[ "$(uname -s)" == "Darwin" ]]; then
        TARGET_TRIPLE="aarch64-apple-darwin"
    else
        TARGET_TRIPLE="aarch64-unknown-linux-musl"
        CARGO_LINKER_ENV_VAR="CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER"
    fi
    ;;
*)
    echo "Unsupported architecture: $ARCHITECTURE" >&2
    exit 1
    ;;
esac

[ -z "${DO_PDF+x}" ] && DO_PDF=false
if [[ $DO_PDF == true ]]; then
    echo "NOT BUILDING AGNO WITH PDF SUPPORT YET"
    exit 1
    # 3) Make Cargo use the C++ linker driver for this target
    export ${CARGO_LINKER_ENV_VAR}="${MUSL_PREFIX}-linux-musl-g++"

    # 4) Ensure any C/C++ built by Cargo build scripts uses the same musl toolchain
    export CC_${TARGET_TRIPLE//-/_}="${MUSL_PREFIX}-linux-musl-gcc"
    export CXX_${TARGET_TRIPLE//-/_}="${MUSL_PREFIX}-linux-musl-g++"
    export AR_${TARGET_TRIPLE//-/_}="${MUSL_PREFIX}-linux-musl-ar"
    export RANLIB_${TARGET_TRIPLE//-/_}="${MUSL_PREFIX}-linux-musl-ranlib"

    # 5) Tame fortify/assertions that can introduce __*chk and __glibcxx_assert_fail
    export CFLAGS_${TARGET_TRIPLE//-/_}="-O2 -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0"
    export CXXFLAGS_${TARGET_TRIPLE//-/_}="-O2 -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -D_GLIBCXX_NO_ASSERTIONS -U_GLIBCXX_ASSERTIONS -D_GLIBCXX_ASSERTIONS=0"
    export CFLAGS="-O2 -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0"
    export CXXFLAGS="-O2 -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -D_GLIBCXX_NO_ASSERTIONS -U_GLIBCXX_ASSERTIONS -D_GLIBCXX_ASSERTIONS=0"

    # 6) Tell pdfium-render to use your prebuilt PDFium (and not fetch/build anything)
    export PDFIUM_STATIC_LIB_PATH="/lib/libpdfium" # must contain libpdfium.a

    # 7) Link settings for Rust (musl dynamic, force-link pdfium + stdc++)
    RUSTFLAGS_COMMON="-C target-feature=-crt-static -C link-arg=-Wl,--no-as-needed"
    RUSTFLAGS_LIBS="-C link-arg=-lpdfium"
    # RUSTFLAGS_LIBS="-C link-arg=-lpdfium -C link-arg=-lc++ link-arg=-lstdc++ -C link-arg=-lm -C link-arg=-lpthread -C link-arg=-ldl -C link-arg=-latomic"

    export RUSTFLAGS="${RUSTFLAGS_COMMON} ${RUSTFLAGS_LIBS}"
else
    RUSTFLAGS_LIBS=""
fi

# 9) Build the Rust static library that links PDFium statically
# Ensure Cargo.toml:
cargo build --release --target "${TARGET_TRIPLE}"

# 11) Copy the resulting static library to a known location
cp "target/${TARGET_TRIPLE}/release/libagno.a" "$1"
