#!/usr/bin/env bash
# published-port smoke for the single image (#1891).
#
# runs the image the way the quickstart tells people to: its default command
# (`rolter easy-up`) with both ports published through `-p`, then curls each
# plane from the host. a bind on the container's loopback passes every check made
# from inside the container and answers nothing through a published port, which
# is how #1891 shipped while `rolter --version` stayed green.
#
# it also checks the other half of that contract: with no ROLTER_ADMIN_TOKEN and
# no ROLTER_ALLOW_OPEN_MODE the image refuses to start, so binding every
# interface did not reopen #970. no secrets are needed; the admin token below is
# a throwaway minted for this run.
#
# usage: image-smoke.sh <image> [platform]
#   platform  e.g. linux/arm64, for an image run under emulation
set -euo pipefail

image="${1:?usage: image-smoke.sh <image> [platform]}"
platform_args=()
if [ -n "${2:-}" ]; then
  platform_args=(--platform "$2")
fi
gateway_port="${SMOKE_GATEWAY_PORT:-14000}"
control_port="${SMOKE_CONTROL_PORT:-14001}"
# generous: an arm64 image under qemu starts several times slower
tries="${SMOKE_TRIES:-90}"
work="$(mktemp -d)"
containers=()

# a container still present here is one a failed check left behind: show why
cleanup() {
  for name in ${containers[@]+"${containers[@]}"}; do
    if docker inspect "$name" >/dev/null 2>&1; then
      echo "== logs: $name =="
      docker logs "$name" 2>&1 | tail -n 60 || true
      docker rm -f "$name" >/dev/null 2>&1 || true
    fi
  done
  rm -rf "$work"
}
trap cleanup EXIT

# poll a URL until it returns 2xx, bounded. args: name url
wait_http() {
  local name="$1" url="$2"
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

# start the image detached with both ports published on the host's loopback.
# args: container-name, then any extra `docker run` flags
start() {
  local name="$1"
  shift
  containers+=("$name")
  docker run -d --name "$name" ${platform_args[@]+"${platform_args[@]}"} \
    -p "127.0.0.1:${gateway_port}:4000" -p "127.0.0.1:${control_port}:4001" \
    "$@" "$image" >/dev/null
}

stop() {
  docker rm -f "$1" >/dev/null
}

echo "== open mode with no acknowledgement: refuses to start =="
refused="rolter-image-smoke-refused-$$"
start "$refused"
for _ in $(seq 1 "$tries"); do
  if [ "$(docker inspect -f '{{.State.Running}}' "$refused")" = "false" ]; then
    break
  fi
  sleep 2
done
if [ "$(docker inspect -f '{{.State.Running}}' "$refused")" != "false" ]; then
  echo "FAILED: the image kept running with no ROLTER_ADMIN_TOKEN and no ROLTER_ALLOW_OPEN_MODE" >&2
  exit 1
fi
if [ "$(docker inspect -f '{{.State.ExitCode}}' "$refused")" = "0" ]; then
  echo "FAILED: the image exited 0 instead of refusing" >&2
  exit 1
fi
docker logs "$refused" >"$work/refused.log" 2>&1
grep -q 'refusing to start' "$work/refused.log"
grep -q 'ROLTER_ALLOW_OPEN_MODE=1' "$work/refused.log"
echo "refused as expected"
stop "$refused"

echo "== open mode, acknowledged: both planes answer through the published ports =="
open="rolter-image-smoke-open-$$"
start "$open" -e ROLTER_ALLOW_OPEN_MODE=1
wait_http gateway "http://127.0.0.1:${gateway_port}/healthz"
wait_http control "http://127.0.0.1:${control_port}/healthz"

echo "-- gateway: fake-llm chat completion with the bundled dev key --"
# the public local-dev virtual key from rolter.example.toml, which the image
# ships as /app/rolter.toml; the quickstart presents the same one
dev_key="sk-rolter-dev"
curl -fsS "http://127.0.0.1:${gateway_port}/v1/chat/completions" \
  -H "Authorization: Bearer ${dev_key}" \
  -H 'content-type: application/json' \
  -d '{"model":"fake-llm","messages":[{"role":"user","content":"hi"}]}' \
  -o "$work/chat.json"
cat "$work/chat.json"
echo
grep -q '"choices"' "$work/chat.json"

echo "-- control: the dashboard is served --"
curl -fsS "http://127.0.0.1:${control_port}/" -o "$work/index.html"
grep -qi '<html' "$work/index.html"
stop "$open"

echo "== closed with an admin token: answers, and the token is enforced =="
closed="rolter-image-smoke-closed-$$"
token="smoke-$(date +%s)-$$"
start "$closed" -e "ROLTER_ADMIN_TOKEN=${token}"
wait_http gateway "http://127.0.0.1:${gateway_port}/healthz"
wait_http control "http://127.0.0.1:${control_port}/healthz"
status="$(curl -sS -o /dev/null -w '%{http_code}' "http://127.0.0.1:${control_port}/internal/snapshot")"
if [ "$status" != "401" ]; then
  echo "FAILED: /internal/snapshot answered $status with no token, expected 401" >&2
  exit 1
fi
curl -fsS "http://127.0.0.1:${control_port}/internal/snapshot" \
  -H "Authorization: Bearer ${token}" -o "$work/snapshot.json"
grep -q '"config"' "$work/snapshot.json"
stop "$closed"

echo "ALL IMAGE SMOKE CHECKS PASSED"
