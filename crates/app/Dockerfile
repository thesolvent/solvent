# The resolver server. Build context is the workspace root — cargo resolves the whole workspace,
# and `git` is needed for the walletkit git dependency.
FROM rust:1-slim AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends git pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release -p solvent --bin solvent

FROM debian:stable-slim
# TLS roots: the price feed and the UniswapX Orders API are both HTTPS.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/solvent /usr/local/bin/solvent

# The server reads `<SOLVENT_CONFIG>.toml`, a token list, and writes two durable stores. Mount a
# volume at /data and point the config's `database_url` and `wallet_state_db` inside it, or an
# unclean restart loses in-flight fills and the registry has to re-scan from `registry_start_block`.
WORKDIR /app
ENV SOLVENT_CONFIG=/app/config/solvent \
    PORT=8080
VOLUME ["/data"]
EXPOSE 8080

# Liveness only — the process answers. Readiness is `/readyz`, which also fails when the order
# feed has gone stale; point the orchestrator's readiness probe at that one, not this. Restarting
# on a stale feed is wrong: the cause is usually upstream and a restart will not fix it.
HEALTHCHECK --interval=15s --timeout=3s --start-period=30s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${PORT:-8080}/healthz" || exit 1

ENTRYPOINT ["solvent"]
