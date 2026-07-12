#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
compose=(docker compose -f "$root/integration/ollama/docker-compose.yml")
port=${ROLTER_OLLAMA_PORT:-4010}

cleanup() {
    "${compose[@]}" down
}
trap cleanup EXIT

"${compose[@]}" up -d --build ollama gateway
until "${compose[@]}" exec -T ollama ollama list >/dev/null 2>&1; do sleep 1; done
"${compose[@]}" exec -T ollama ollama pull qwen2.5:0.5b

until curl --fail --silent "http://localhost:$port/healthz" >/dev/null; do sleep 1; done
python3 "$root/integration/ollama/smoke.py" "http://localhost:$port"

