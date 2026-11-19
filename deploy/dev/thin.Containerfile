# Thin runtime image for deploy/dev/kind-dev.sh (OPS-017): one prebuilt service binary on the same
# distroless base as images/services/Containerfile. Build context: the directory holding the binary.
#   podman build -f deploy/dev/thin.Containerfile --build-arg BIN=aurora-api -t localhost/aurora-dev/aurora-api:dev <dir>
FROM gcr.io/distroless/cc-debian12:nonroot
ARG BIN
COPY ${BIN} /usr/local/bin/aurora-service
# The builtin app packages, unsigned (P5-UX-6, D-326(d)): kind-dev.sh puts them in the build context's
# `plugins/` for the API image only (the others get an empty directory). They go where the release image
# puts its signed ones, the builtin directory (`plugins.builtin_dir`, P5-POLISH-3, D-381, SEC-136), so a dev
# install behaves like a release one: with `config.plugins.allow_unsigned` of values-dev.yaml the API takes
# an unsigned builtin as approved (audit `plugin.auto_approved`, reason `builtin_dev_unsigned`).
COPY plugins /usr/share/aurora/plugins
ENTRYPOINT ["/usr/local/bin/aurora-service"]
