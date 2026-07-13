#!/usr/bin/env bash
# Run the engine suite (vLLM API simulator, or a real vLLM/SGLang dummy-weight
# server) through the local rolter binary.
set -euo pipefail

engine=${1:?usage: integration/engines/run.sh <sim|vllm|sglang> [--bench]}
mode=${2:-smoke}
case "$engine" in sim|vllm|sglang) ;; *) echo "unknown engine: $engine" >&2; exit 2 ;; esac
case "$mode" in smoke|--bench) ;; *) echo "unknown mode: $mode" >&2; exit 2 ;; esac

command -v docker >/dev/null || { echo "docker is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
compose=(docker compose -f "$root/docker/docker-compose.engines.yml" --profile "$engine")
engine_1_port=8000
engine_2_port=8001
[[ "$engine" == "sglang" ]] && engine_1_port=30000 && engine_2_port=30001
rolter_port=${ROLTER_ENGINE_TEST_PORT:-4010}
artifacts=${ROLTER_ENGINE_ARTIFACTS:-"$root/artifacts/engines/$engine"}
mkdir -p "$artifacts"
config=$(mktemp)
gateway_pid=""

cleanup() {
  status=$?
  if [[ -n "$gateway_pid" ]]; then kill "$gateway_pid" 2>/dev/null || true; wait "$gateway_pid" 2>/dev/null || true; fi
  "${compose[@]}" logs --no-color >"$artifacts/$engine.log" 2>&1 || true
  "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  rm -f "$config"
  exit "$status"
}
trap cleanup EXIT

sed -e "s/__ROLTER_PORT__/$rolter_port/g" \
  -e "s/__ENGINE_1_PORT__/$engine_1_port/g" \
  -e "s/__ENGINE_2_PORT__/$engine_2_port/g" \
  "$root/integration/engines/rolter-dummy.toml.in" >"$config"

"${compose[@]}" up -d
(cd "$root" && cargo build -p rolter-gateway)
"$root/target/debug/rolter-gateway" --config "$config" >"$artifacts/rolter.log" 2>&1 &
gateway_pid=$!

if [[ "$mode" == "--bench" ]]; then
  python3 "$root/integration/engines/smoke.py" --engine-url "http://127.0.0.1:$engine_1_port" --engine-url "http://127.0.0.1:$engine_2_port" --gateway-url "http://127.0.0.1:$rolter_port"
  python3 "$root/integration/engines/bench.py" --engine-url "http://127.0.0.1:$engine_1_port" --gateway-url "http://127.0.0.1:$rolter_port" --output "$artifacts/non-streaming.json"
  python3 "$root/integration/engines/bench.py" --engine-url "http://127.0.0.1:$engine_1_port" --gateway-url "http://127.0.0.1:$rolter_port" --stream --output "$artifacts/streaming.json"
else
  python3 "$root/integration/engines/smoke.py" --engine-url "http://127.0.0.1:$engine_1_port" --engine-url "http://127.0.0.1:$engine_2_port" --gateway-url "http://127.0.0.1:$rolter_port"
fi
