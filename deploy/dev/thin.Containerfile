# Thin runtime image for deploy/dev/kind-dev.sh (OPS-017): one prebuilt service binary on the same
# distroless base as images/services/Containerfile. Build context: the directory holding the binary.
#   podman build -f deploy/dev/thin.Containerfile --build-arg BIN=aurora-api -t localhost/aurora-dev/aurora-api:dev <dir>
FROM gcr.io/distroless/cc-debian12:nonroot
ARG BIN
COPY ${BIN} /usr/local/bin/aurora-service
ENTRYPOINT ["/usr/local/bin/aurora-service"]
