#!/usr/bin/env bash
set -euo pipefail

ARCHITECTURE="${ARCHITECTURE:-amd64}"

case "$ARCHITECTURE" in
amd64)
    MUSL_PREFIX="x86_64"
    TARGET_TRIPLE="x86_64-unknown-linux-musl"
    CARGO_LINKER_ENV_VAR="CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER"
    ;;
arm64)
    MUSL_PREFIX="aarch64"
    TARGET_TRIPLE="aarch64-unknown-linux-musl"
    CARGO_LINKER_ENV_VAR="CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER"
    ;;
*)
    echo "Unsupported architecture: $ARCHITECTURE" >&2
    exit 1
    ;;
esac

MUSL_VERSION="${MUSL_PREFIX}-linux-musl-cross"
mkdir -p /opt/musl
curl -L "https://musl.cc/${MUSL_VERSION}.tgz" | tar xz -C /opt/musl

for BINARY in gcc g++ ar nm strip ranlib; do
    ln -sf /opt/musl/${MUSL_VERSION}/bin/${MUSL_PREFIX}-linux-musl-${BINARY} /usr/local/bin/${MUSL_PREFIX}-linux-musl-${BINARY}
    ln -sf /opt/musl/${MUSL_VERSION}/bin/${MUSL_PREFIX}-linux-musl-${BINARY} /usr/local/bin/${MUSL_PREFIX}-linux-gnu-${BINARY}
done

# 2) Rust toolchain and target
if ! command -v rustup >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    . "$HOME/.cargo/env"
fi
rustup target add "${TARGET_TRIPLE}"
