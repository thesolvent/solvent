#!/usr/bin/env bash
#
# Run a command against a throwaway Postgres in Docker, exporting TEST_DATABASE_URL
# for it. Reused by the registry store integration tests and later end-to-end tests.
#
#   scripts/with-postgres.sh cargo test -p solvent-adapters --test pg_store
#
# The container is named per-PID and force-removed on exit (success, failure, or Ctrl-C).

set -euo pipefail

if [[ $# -eq 0 ]]; then
    echo "usage: $0 <command> [args...]" >&2
    exit 2
fi

readonly container="solvent-test-pg-$$"
readonly port="${PG_PORT:-55432}"
export TEST_DATABASE_URL="postgres://postgres:postgres@127.0.0.1:${port}/postgres"

cleanup() { docker rm --force "$container" >/dev/null 2>&1 || true; }
trap cleanup EXIT

echo "starting Postgres (container ${container}, port ${port})…" >&2
docker run --detach --name "$container" \
    --env POSTGRES_PASSWORD=postgres \
    --publish "${port}:5432" \
    postgres:16-alpine >/dev/null

echo "waiting for Postgres to accept connections…" >&2
ready=false
for _ in $(seq 1 30); do
    if docker exec "$container" pg_isready --username postgres >/dev/null 2>&1; then
        ready=true
        break
    fi
    sleep 1
done

if [[ "$ready" != true ]]; then
    echo "Postgres did not become ready in time" >&2
    exit 1
fi

# Run the command in this shell (not `exec`) so the EXIT trap still fires and
# removes the container. `set -e` propagates a non-zero exit code, trap and all.
"$@"
