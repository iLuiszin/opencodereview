#!/bin/bash
set -uo pipefail
repo=$(cd "${1:-.}" && pwd)
network=wp-test-$$
database=wp-test-pg-$$
cleanup() {
  docker rm -f "$database" >/dev/null 2>&1
  docker network rm "$network" >/dev/null 2>&1
}
trap cleanup EXIT
docker network create "$network" >/dev/null || exit 1
docker run -d --name "$database" --network "$network" -e POSTGRES_USER=ocr -e POSTGRES_PASSWORD=dev postgres:17 >/dev/null || exit 1
for _ in $(seq 1 30); do
  docker exec "$database" pg_isready -U ocr >/dev/null 2>&1 && break
  sleep 1
done
docker run --rm --network "$network" -v "$repo:/app" -w /app \
  -v wp-cargo-registry:/usr/local/cargo/registry -v wp-cargo-target:/app/target \
  -e CARGO_BUILD_JOBS=6 -e "OCR_TEST_DATABASE_URL=postgres://ocr:dev@$database:5432/postgres" \
  rust:1.98.1-bookworm sh -c 'apt-get -qq update >/dev/null && apt-get -qq install -y pkg-config libssl-dev git >/dev/null && cargo test --locked --lib'
