# Dev builder for deploy/dev/kind-dev.sh (OPS-017): the same Rust image and linker as images/services/Containerfile.
# The source is bind-mounted and CARGO_TARGET_DIR is a named volume, so a rebuild compiles only what changed.
ARG RUST_IMAGE=docker.io/library/rust:1-bookworm
FROM ${RUST_IMAGE}
RUN apt-get update && apt-get install -y --no-install-recommends clang lld \
    && rm -rf /var/lib/apt/lists/*
ENV SQLX_OFFLINE=true CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 \
    RUSTFLAGS="-C link-arg=-fuse-ld=lld" RUSTUP_TOOLCHAIN=stable CARGO_TARGET_DIR=/target
WORKDIR /src
# RUSTUP_TOOLCHAIN overrides the repo's rust-toolchain.toml (wasm target, components) that service builds do not need.
