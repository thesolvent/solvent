# Build context is the workspace root (cargo resolves the whole workspace, so `git` is needed to
# fetch git dependencies).
FROM rust:1-slim AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends git pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release -p solvent --bin solvent

FROM debian:stable-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/solvent /usr/local/bin/solvent
EXPOSE 8080
ENTRYPOINT ["solvent"]
