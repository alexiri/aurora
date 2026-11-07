# syntax=docker/dockerfile:1
# Development IdP image (P1-005, RFE-004): `aurora-dev-idp` only, for the kind e2e (scripts/e2e/kind.sh).
# DEVELOPMENT AND TESTS ONLY: it is not part of the release image set (images/services/Containerfile) and the
# Helm chart enables it only with `devIdp.enabled` on a loopback `externalUrl`. The kind *dev* stack uses the
# thin image of deploy/dev/thin.Containerfile instead (kind-dev.sh builds the binary in the builder container).
#   podman build -f deploy/dev/dev-idp.Containerfile -t localhost/aurora-e2e/aurora-dev-idp:e2e .
ARG RUST_IMAGE=docker.io/library/rust:1-bookworm
FROM ${RUST_IMAGE} AS build
WORKDIR /build
ENV SQLX_OFFLINE=true CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0
COPY . .
RUN rm -f rust-toolchain.toml
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    cargo build --release --locked -p aurora-testkit --bin aurora-dev-idp \
    && mkdir /out && cp target/release/aurora-dev-idp /out/

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /out/aurora-dev-idp /usr/local/bin/aurora-dev-idp
EXPOSE 4107
ENTRYPOINT ["/usr/local/bin/aurora-dev-idp"]
