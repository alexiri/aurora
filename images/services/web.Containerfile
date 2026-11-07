# Next.js standalone build. Context: repo root.
#   podman build -f images/services/web.Containerfile -t aurora-web .
FROM docker.io/library/node:22-slim AS build
ENV NEXT_TELEMETRY_DISABLED=1
RUN npm install -g pnpm@10
WORKDIR /app
COPY web/package.json web/pnpm-lock.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY web/ ./
RUN mkdir -p public && pnpm build

FROM docker.io/library/node:22-slim AS web
ENV NODE_ENV=production NEXT_TELEMETRY_DISABLED=1 PORT=4100 HOSTNAME=0.0.0.0
WORKDIR /app
COPY --from=build --chown=node:node /app/.next/standalone ./
COPY --from=build --chown=node:node /app/.next/static ./.next/static
COPY --from=build --chown=node:node /app/public ./public
USER node
EXPOSE 4100
CMD ["node", "server.js"]
