# Default features for non-GPU builds (CI, Docker)
default_features := "jpeg,png,webp,pdf"

# Build the static library (release)
# Examples:
#   just build libagno.a
#   just build libagno.a --target aarch64-unknown-linux-gnu
#   just build libagno.a --features gpu,jpeg,png,webp,pdf
build output='libagno.a' *args:
    #!/usr/bin/env bash
    set -euo pipefail
    for arg in {{args}}; do
        case "$prev" in
            --target)   export TARGET_TRIPLE="$arg" ;;
            --features) export AGNO_FEATURES="$arg" ;;
        esac
        prev="$arg"
    done
    ./build/sh/build-agno.bash "{{output}}"

# Build via Docker (cross-compilation)
# Examples:
#   just docker-build arm64
#   just docker-build amd64 --pdf
docker-build arch='arm64' *args:
    #!/usr/bin/env bash
    set -euo pipefail
    pdf_flag="false"
    for arg in {{args}}; do
        case "$arg" in
            --pdf) pdf_flag="true" ;;
        esac
    done
    docker build -f build/Dockerfile \
        --platform "linux/{{arch}}" \
        --build-arg "TARGETARCH={{arch}}" \
        --build-arg "PDF=$pdf_flag" \
        --output "type=local,dest=." .

# Run all tests
# Examples:
#   just test
#   just test --release
#   just test jpeg_roundtrip
test *args:
    #!/usr/bin/env bash
    set -euo pipefail
    extra_args=()
    filter=""
    for arg in {{args}}; do
        case "$arg" in
            --release|--nocapture) extra_args+=("$arg") ;;
            --*)                  extra_args+=("$arg") ;;
            *)                    filter="$arg" ;;
        esac
    done
    if [ -n "$filter" ]; then
        cargo test -p agno "${extra_args[@]}" -- "$filter"
    else
        cargo test -p agno "${extra_args[@]}"
    fi

# Run clippy and format check
# Examples:
#   just lint
#   just lint --fix
lint *args:
    #!/usr/bin/env bash
    set -euo pipefail
    fix=false
    for arg in {{args}}; do
        case "$arg" in
            --fix) fix=true ;;
        esac
    done
    if [ "$fix" = true ]; then
        cargo fmt
        cargo clippy --workspace --fix --allow-dirty --allow-staged -- -D warnings
    else
        cargo fmt -- --check
        cargo clippy --workspace -- -D warnings
    fi

# Format code
fmt:
    cargo fmt

# Check formatting without modifying
check-fmt:
    cargo fmt -- --check

# Run clippy
clippy *args:
    cargo clippy --workspace {{args}} -- -D warnings

# Build debug (fast compile, no optimizations)
check:
    cargo check --workspace

# Run the CLI
# Examples:
#   just run exif photo.heic
#   just run convert input.arw output.jpg
run *args:
    cargo run -p agno -- {{args}}

# Clean build artifacts
clean:
    cargo clean
