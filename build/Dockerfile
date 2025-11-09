ARG TARGETARCH="arm64"
ARG PDF=false

FROM debian:bookworm-slim AS build
ARG TARGETARCH
ENV ARCHITECTURE=${TARGETARCH}

ARG PDF
ENV DO_PDF=${PDF}

# Deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    git curl ca-certificates python3 python3-venv gnupg lsb-release \
    ninja-build pkg-config cmake build-essential g++ \
    && rm -rf /var/lib/apt/lists/*;

ENV DEPOT_TOOLS=/opt/depot_tools
ENV PATH="${DEPOT_TOOLS}:$PATH"
ENV ENABLE_V8=false

WORKDIR /src

COPY ./build/sh/installDeps.bash /build/installDeps.bash
RUN /build/installDeps.bash;

# Fetch pdfium
COPY ./build/sh/checkoutPdfium.bash /build/checkoutPdfium.bash
RUN if [ $DO_PDF == true ]; then /build/checkoutPdfium.bash; fi;

# Build pdfium
COPY ./build/sh/buildPdfium.bash /build/buildPdfium.bash
RUN if [ $DO_PDF == true ]; then /build/buildPdfium.bash; fi;

RUN if [ $DO_PDF == true ]; then mkdir -p /lib/libpdfium && cp /src/pdfium/out/Static/obj/libpdfium.a /lib/libpdfium; fi;

WORKDIR /src/agno
COPY . .

COPY ./build/sh/buildAgno.bash /build/buildAgno.bash
RUN /build/buildAgno.bash;

FROM --platform=linux/${TARGETARCH} scratch
COPY --from=build /lib/libagno/libagno.a /libagno.a
