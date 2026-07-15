#!/usr/bin/env bash
# full-stack docker compose smoke (ROL-245).
#
# brings up the production-shaped topology (postgres, redis, clickhouse, gateway,
# control) via docker-compose.yml + the CI overlay, waits for both health
# endpoints, then exercises the gateway (models + fake-llm chat, non-streaming
# and SSE) and the control-plane snapshot path. no provider secrets are needed.
#
# always dumps compose logs and tears the stack down (including volumes) on exit,
# so the job leaves nothing behind whether it passes or fails.
set -euo pipefail

cd "$(dirname "$0")/.."   # -> docker/
COMPOSE=(docker compose -f docker-compose.yml -f docker-compose.ci.yml)

cleanup() {
  echo "== compose ps =="
  "${COMPOSE[@]}" ps || true
  echo "== compose logs =="
  "${COMPOSE[@]}" logs --no-color --timestamps || true
  echo "== tearing down =="
  "${COMPOSE[@]}" down -v --remove-orphans || true
}
trap cleanup EXIT

echo "== building + starting stack =="
"${COMPOSE[@]}" up -d --build

# poll a URL until it returns 2xx, bounded. args: name url [tries]
wait_http() {
  local name="$1" url="$2" tries="${3:-90}"
  for _ in $(seq 1 "$tries"); do
    if curl -fsS -o /dev/null "$url" 2>/dev/null; then
      echo "$name is up ($url)"
      return 0
    fi
    sleep 2
  done
  echo "FAILED: timed out waiting for $name at $url" >&2
  return 1
}

wait_http gateway http://127.0.0.1:4000/healthz
wait_http control http://127.0.0.1:4001/healthz

echo "== gateway: GET /v1/models (expects built-in fake-llm) =="
curl -fsS http://127.0.0.1:4000/v1/models | tee /tmp/models.json; echo
grep -q '"data"' /tmp/models.json
grep -q 'fake-llm' /tmp/models.json

echo "== gateway: fake-llm chat completion (non-streaming) =="
curl -fsS http://127.0.0.1:4000/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"fake-llm","messages":[{"role":"user","content":"hi"}]}' \
  | tee /tmp/chat.json; echo
grep -q '"choices"' /tmp/chat.json

echo "== gateway: fake-llm chat completion (streaming SSE) =="
# write to a file rather than piping into grep -q: grep exits on the first match
# and closes the pipe, which under `set -o pipefail` surfaces as SIGPIPE (141)
curl -fsS -N --max-time 30 http://127.0.0.1:4000/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"fake-llm","stream":true,"messages":[{"role":"user","content":"hi"}]}' \
  -o /tmp/chat-sse.txt
cat /tmp/chat-sse.txt
grep -q '^data:' /tmp/chat-sse.txt

echo "== control: GET /internal/snapshot (postgres-backed, after DB is ready) =="
curl -fsS http://127.0.0.1:4001/internal/snapshot | tee /tmp/snap.json; echo
grep -q '"version"' /tmp/snap.json
grep -q '"config"' /tmp/snap.json

echo "ALL SMOKE CHECKS PASSED"
