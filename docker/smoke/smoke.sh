#!/usr/bin/env bash
# full-stack docker compose smoke (ROL-245).
#
# brings up the production-shaped topology (postgres, redis, clickhouse, gateway,
# control) via docker-compose.yml + the CI overlay, waits for both health
# endpoints, then exercises the gateway (models + fake-llm chat, non-streaming
# and SSE), the control-plane snapshot path, and that a route created through the
# control plane's API reaches the gateway. no provider secrets are needed.
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

echo "== control -> gateway: a route created through the control plane reaches the gateway =="
# the stack is open, so the management API needs no token. the gateway polls
# /internal/snapshot (and listens on redis), so the route shows up in its model
# list without a restart. a gateway that ignored the control plane, as this one
# did until #1890, would never list it
json_id() { python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])'; }
post() { curl -fsS -H 'content-type: application/json' -d "$2" "http://127.0.0.1:4001$1"; }
org=$(post /api/v1/orgs '{"name":"smoke","slug":"smoke"}' | json_id)
team=$(post "/api/v1/orgs/$org/teams" '{"name":"smoke"}' | json_id)
project=$(post "/api/v1/teams/$team/projects" '{"name":"smoke"}' | json_id)
# a route needs a target on a known provider, or the snapshot leaves it out. the
# provider never has to answer: the check is that the route arrives
provider=$(post "/api/v1/orgs/$org/providers" \
  '{"name":"smoke-upstream","kind":"openai","api_base":"http://smoke-upstream.invalid","api_key_env":"SMOKE_UPSTREAM_KEY"}' | json_id)
route=$(post "/api/v1/projects/$project/routes" '{"model":"smoke-follow","strategy":"round_robin"}' | json_id)
post "/api/v1/routes/$route/targets" "{\"provider_id\":\"$provider\",\"upstream_model\":\"gpt-4o\"}" >/dev/null
followed=0
for _ in $(seq 1 30); do
  if curl -fsS http://127.0.0.1:4000/v1/models | grep -q 'smoke-follow'; then
    followed=1
    break
  fi
  sleep 1
done
if [ "$followed" -ne 1 ]; then
  echo "FAILED: the gateway never listed a route created in the control plane" >&2
  exit 1
fi
echo "gateway lists smoke-follow"

echo "ALL SMOKE CHECKS PASSED"
