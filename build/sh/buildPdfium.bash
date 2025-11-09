#!/bin/bash -eux

cd /src/pdfium
mkdir -p out/Static

if [[ $ARCHITECTURE == "amd64" ]]; then
    export MUSL_PREFIX="x86_64"
    export TARGET_CPU="x64"
elif [[ $ARCHITECTURE == "arm64" ]]; then
    export MUSL_PREFIX="aarch64"
    export TARGET_CPU="arm64"
else
    echo "Unsupported architecture: $ARCHITECTURE"
    exit 1
fi

export ENABLE_V8=false

gn gen out/Static --args=" \
  is_clang=false \
  is_debug=false \
  is_component_build=false \
  is_musl=true \
  use_sysroot=false \
  pdf_is_complete_lib=true \
  use_glib=false \
  use_pkg_config=false \
  target_os=\"linux\" \
  target_cpu=\"${TARGET_CPU}\" \
  use_gtk=false \
  use_x11=false \
  use_udev=false \
  pdf_is_standalone=true \
  pdf_enable_v8=false \
  pdf_enable_xfa=false \
  pdf_use_partition_alloc=false \
  treat_warnings_as_errors=false \
  use_custom_libcxx=false \
  use_custom_libcxx_for_host=false \
  cc=\"${MUSL_PREFIX}-linux-musl-gcc\" \
  cxx=\"${MUSL_PREFIX}-linux-musl-g++\" \
  nm=\"${MUSL_PREFIX}-linux-musl-nm\" \
  strip=\"${MUSL_PREFIX}-linux-musl-strip\" \
  ld=\"${MUSL_PREFIX}-linux-musl-g++\" \
  ldflags = [ \"-static\" ] \
  extra_ldflags = [ \"-static\" ] \
"

# Build with limited parallelism for stability
ninja -C out/Static -v pdfium
