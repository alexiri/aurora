# Thin runtime image for deploy/dev/kind-dev.sh (OPS-017): one prebuilt service binary on the same
# distroless base as images/services/Containerfile. Build context: the directory holding the binary.
#   podman build -f deploy/dev/thin.Containerfile --build-arg BIN=aurora-api -t localhost/aurora-dev/aurora-api:dev <dir>
FROM gcr.io/distroless/cc-debian12:nonroot
ARG BIN
COPY ${BIN} /usr/local/bin/aurora-service
# The builtin app packages, unsigned (P5-UX-6, D-326(d)): kind-dev.sh puts them in the build context's
# `plugins/` for the API image only (the others get an empty directory). The API scans this
# directory (`plugins.dir`, the chart's `plugins.source: none` mounts nothing over it) and, with
# `config.plugins.allow_unsigned` of values-dev.yaml, takes them as pending approval.
COPY plugins /var/lib/aurora/plugins
ENTRYPOINT ["/usr/local/bin/aurora-service"]
