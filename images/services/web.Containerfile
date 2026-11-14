# syntax=docker/dockerfile:1
# Next.js standalone build. Context: repo root.
#   podman build -f images/services/web.Containerfile -t aurora-web .
# (FIX-103) The base image is pinned by digest and pnpm by exact version (scripts/guards/image-pins.sh);
# bump the tag and the digest together.
ARG NODE_IMAGE=docker.io/library/node:22-slim@sha256:e9f565d32859f0cbfb52125eafe8be39390f24dac302dc3cdf566e8ba3ab21ee
ARG PNPM_VERSION=10.34.9

FROM ${NODE_IMAGE} AS build
ARG PNPM_VERSION
ENV NEXT_TELEMETRY_DISABLED=1
RUN npm install -g pnpm@${PNPM_VERSION}
WORKDIR /app
COPY web/package.json web/pnpm-lock.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY web/ ./
RUN mkdir -p public && pnpm build

FROM ${NODE_IMAGE} AS web
ENV NODE_ENV=production NEXT_TELEMETRY_DISABLED=1 PORT=4100 HOSTNAME=0.0.0.0
WORKDIR /app
COPY --from=build --chown=node:node /app/.next/standalone ./
COPY --from=build --chown=node:node /app/.next/static ./.next/static
COPY --from=build --chown=node:node /app/public ./public
USER node
EXPOSE 4100
CMD ["node", "server.js"]
