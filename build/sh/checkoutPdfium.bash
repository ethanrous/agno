#!/bin/bash -eux

# depot_tools
git clone https://chromium.googlesource.com/chromium/tools/depot_tools.git ${DEPOT_TOOLS}
export PATH="${DEPOT_TOOLS}:$PATH"

if [[ $ARCHITECTURE == "amd64" ]]; then
    export MUSL_PREFIX="x86_64"
elif [[ $ARCHITECTURE == "arm64" ]]; then
    export MUSL_PREFIX="aarch64"
else
    echo "Unsupported architecture: $ARCHITECTURE"
    exit 1
fi

MUSL_VERSION="${MUSL_PREFIX}-linux-musl-cross"
mkdir -p /opt/musl
curl -L "https://musl.cc/${MUSL_VERSION}.tgz" | tar xz -C /opt/musl

for BINARY in gcc g++ ar nm strip ranlib; do
    ln -sf /opt/musl/${MUSL_VERSION}/bin/${MUSL_PREFIX}-linux-musl-${BINARY} /usr/local/bin/${MUSL_PREFIX}-linux-musl-${BINARY}
    ln -sf /opt/musl/${MUSL_VERSION}/bin/${MUSL_PREFIX}-linux-musl-${BINARY} /usr/local/bin/${MUSL_PREFIX}-linux-gnu-${BINARY}
done

export ENABLE_V8=false

# Fetch pdfium
gclient config --unmanaged https://pdfium.googlesource.com/pdfium.git --custom-var checkout_configuration=minimal
echo "target_os = [ 'linux' ]" >>.gclient

# for FOLDER in pdfium pdfium/build pdfium/v8 pdfium/third_party/libjpeg_turbo pdfium/base/allocator/partition_allocator; do
#     if [ -e "$FOLDER" ]; then
#         git -C $FOLDER reset --hard
#         git -C $FOLDER clean -df
#     fi
# done

gclient sync --verbose
# gclient sync -r origin/chromium/7442 --no-history --shallow
