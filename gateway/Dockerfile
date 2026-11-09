# syntax=docker/dockerfile:1

FROM rust:1.99-slim-trixie AS build
WORKDIR /src

# Build dependencies on their own layer so source changes don't rebuild them.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && touch src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY migrations migrations
COPY src src
COPY deploy deploy
RUN touch src/main.rs src/lib.rs && cargo build --release --locked

# Local stand-in for providers and the facilitator (docker compose only; never deployed).
FROM debian:trixie-slim AS mock
COPY --from=build /src/target/release/enclavo-mock /usr/local/bin/enclavo-mock
USER 10001
EXPOSE 4000
ENTRYPOINT ["/usr/local/bin/enclavo-mock"]

# The gateway. Last stage, so it is what `fly deploy` builds by default.
FROM debian:trixie-slim AS gateway
# CA roots for TLS to providers and the facilitator.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home enclavo
COPY --from=build /src/target/release/enclavo-gateway /usr/local/bin/enclavo-gateway
COPY --from=build /src/target/release/enclavo-admin /usr/local/bin/enclavo-admin
COPY deploy/*.toml /etc/enclavo/
USER enclavo
EXPOSE 8080 9091
ENTRYPOINT ["/usr/local/bin/enclavo-gateway"]
