#!/usr/bin/env bash
set -euo pipefail

llama_url="${1:-http://127.0.0.1:8080}"
llama_model="${2:?usage: $0 [llama-server-url] <upstream-model-id>}"
rolter_url="http://127.0.0.1:14000"
tmp="$(mktemp -d)"
trap 'kill "${rolter_pid:-}" 2>/dev/null || true; rm -rf "$tmp"' EXIT

curl --fail --silent --show-error "$llama_url/v1/models" >/dev/null
cat >"$tmp/rolter.toml" <<EOF
[[providers]]
name = "local-llama"
kind = "llama_cpp"
api_base = "$llama_url"

[[routes]]
model = "llama-smoke"
strategy = "round_robin"

[[routes.targets]]
provider = "local-llama"
model = "$llama_model"

[server]
host = "127.0.0.1"
port = 14000
EOF

cargo run -q -p rolter-gateway -- --config "$tmp/rolter.toml" >"$tmp/gateway.log" 2>&1 &
rolter_pid=$!
for _ in {1..60}; do
  curl --fail --silent "$rolter_url/healthz" >/dev/null 2>&1 && break
  sleep 1
done
curl --fail --silent --show-error "$rolter_url/v1/models" | grep -q 'llama-smoke'

headers="$tmp/headers"
curl --fail --silent --show-error -D "$headers" \
  -H 'content-type: application/json' \
  -d '{"model":"llama-smoke","prompt":"Reply with OK","max_tokens":8,"temperature":0}' \
  "$rolter_url/v1/completions" | grep -q 'choices'
grep -qi '^x-rolter-provider: local-llama' "$headers"

curl --fail --silent --show-error -N \
  -H 'content-type: application/json' \
  -d '{"model":"llama-smoke","prompt":"Reply with OK","max_tokens":8,"stream":true}' \
  "$rolter_url/v1/completions" | grep -q '^data:'

echo "llama.cpp smoke passed"
