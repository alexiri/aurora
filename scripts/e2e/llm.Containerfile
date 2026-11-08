# Sandbox image for the TEST-017 hosted e2e: the e2e base image (OS + aurora-supervisor, no Node) plus
# the pinned Claude Code CLI at the path the adapter expects. Same install as images/sandbox/Containerfile
# (`sandbox` target) without the developer tool set (chromium etc.).
#   podman build -f scripts/e2e/llm.Containerfile -t localhost/aurora-llm-sandbox:0.1.0 .
ARG BASE=localhost/aurora-sandbox:0.1.0
FROM ${BASE}
USER root
# Must match PINNED_VERSION in services/aurora-supervisor/src/adapters/claude_code/mod.rs.
ARG CLAUDE_CODE_VERSION=2.1.319
# retries/minrate/timeout only tolerate a slow mirror (P2-OPS-1, D-100); gpgcheck and repo verification are untouched.
RUN dnf -y install --setopt=install_weak_deps=False --setopt=retries=10 --setopt=minrate=100 --setopt=timeout=120 nodejs npm \
 && mkdir -p /opt/aurora/harness/claude-code \
 && npm install --prefix /opt/aurora/harness/claude-code --no-audit --no-fund @anthropic-ai/claude-code@${CLAUDE_CODE_VERSION} \
 && /opt/aurora/harness/claude-code/node_modules/.bin/claude --version \
 && dnf clean all && rm -rf /var/cache/dnf /root/.npm
USER 1000:1000
WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/aurora-supervisor", "run"]
