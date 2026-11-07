# VidaiMock v0.4.0 (Apache-2.0) + Aurora's provider config, templates and scenarios baked in at /config
# (decision I-30; see vidaimock/README.md). Dev only. Built by deploy/dev/kind-dev.sh and loaded into kind
# (a ConfigMap cannot hold the nested config directory). Build context: deploy/dev/llm-mock.
FROM ghcr.io/vidaiuk/vidaimock@sha256:c306a4e87637976bfd9034d0275ae6932497b18ee71614c46a0f143f393127c6
COPY vidaimock /config
