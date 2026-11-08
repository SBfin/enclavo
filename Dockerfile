FROM node:24-slim

# Privy's dependencies include native modules that compile on Linux ARM64.
RUN apt-get update \
    && apt-get install -y --no-install-recommends python3 make g++ \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
EXPOSE 3000
