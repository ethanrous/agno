# Build libde265 as a static library.
# Apt only ships a shared-library package; we need .a so it can be merged into libagno.a.
LIBDE265_VERSION=${LIBDE265_VERSION:-1.0.15}
curl -fsSL "https://github.com/strukturag/libde265/releases/download/v${LIBDE265_VERSION}/libde265-${LIBDE265_VERSION}.tar.gz" |
    tar -xz &&
    cmake -S "libde265-${LIBDE265_VERSION}" -B libde265-build \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX=/usr/local \
        -DBUILD_SHARED_LIBS=OFF &&
    cmake --build libde265-build --parallel "$(nproc)" &&
    cmake --install libde265-build &&
    rm -rf "libde265-${LIBDE265_VERSION}" libde265-build

# Build libheif 1.21.2 as a static library.
# Debian apt ships ~1.17.x; libheif-sys requires >= 1.21 (enforced by pkg-config at cargo build time).
# Built against our static libde265 so both can be merged into the final libagno.a.
LIBHEIF_VERSION=${LIBHEIF_VERSION:-1.21.2}
curl -fsSL "https://github.com/strukturag/libheif/releases/download/v${LIBHEIF_VERSION}/libheif-${LIBHEIF_VERSION}.tar.gz" |
    tar -xz &&
    cmake -S "libheif-${LIBHEIF_VERSION}" -B libheif-build \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX=/usr/local \
        -DBUILD_SHARED_LIBS=OFF \
        -DENABLE_PLUGIN_LOADING=OFF \
        -DBUILD_TESTING=OFF \
        -DWITH_EXAMPLES=OFF &&
    cmake --build libheif-build --parallel "$(nproc)" &&
    cmake --install libheif-build &&
    rm -rf "libheif-${LIBHEIF_VERSION}" libheif-build
