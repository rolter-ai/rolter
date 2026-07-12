#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
compose=(docker compose -f "$root/integration/tei/docker-compose.yml")
port=${ROLTER_TEI_PORT:-4013}
trap '"${compose[@]}" down' EXIT

"${compose[@]}" up -d --build
until curl --fail --silent "http://localhost:$port/healthz" >/dev/null; do sleep 1; done

headers=$(mktemp)
trap 'rm -f "$headers"; "${compose[@]}" down' EXIT
body=$(curl --fail --silent --show-error -D "$headers" \
  -H 'content-type: application/json' \
  -d '{"model":"embed-local","input":["hello","world"],"encoding_format":"float"}' \
  "http://localhost:$port/v1/embeddings")
grep -q '"embedding"' <<<"$body"
grep -q '"usage"' <<<"$body"
grep -qi '^x-rolter-provider: tei-local' "$headers"
echo "TEI smoke passed"
