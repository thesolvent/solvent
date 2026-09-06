# Build context is the workspace root (cargo resolves the whole workspace, so `git` is needed to
# fetch git dependencies). Devnet only.
FROM rust:1-slim AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends git \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release -p solvent-devnet --bin faucet

FROM debian:stable-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/faucet /usr/local/bin/faucet
EXPOSE 8080
ENTRYPOINT ["faucet"]
