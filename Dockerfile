FROM node:24-slim AS development

# Privy's dependencies include native modules that compile on Linux ARM64.
RUN apt-get update \
    && apt-get install -y --no-install-recommends python3 make g++ \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
EXPOSE 3000

FROM development AS dependencies
COPY package.json package-lock.json ./
RUN npm ci

FROM dependencies AS builder
# Public browser configuration is embedded by Next.js at build time. Never pass secrets here.
ARG NEXT_PUBLIC_PRIVY_APP_ID
ENV NEXT_PUBLIC_PRIVY_APP_ID=$NEXT_PUBLIC_PRIVY_APP_ID \
    NEXT_TELEMETRY_DISABLED=1 \
    RAYON_NUM_THREADS=2 \
    TOKIO_WORKER_THREADS=2
COPY src ./src
COPY next.config.ts tsconfig.json ./
RUN test -n "$NEXT_PUBLIC_PRIVY_APP_ID" && npm run build

FROM node:24-slim AS production
WORKDIR /app
ENV NODE_ENV=production \
    NEXT_TELEMETRY_DISABLED=1 \
    HOSTNAME=0.0.0.0 \
    PORT=3000
COPY --from=builder --chown=node:node /app/.next/standalone ./
COPY --from=builder --chown=node:node /app/.next/static ./.next/static
USER node
EXPOSE 3000
CMD ["node", "server.js"]
