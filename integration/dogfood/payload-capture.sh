#!/usr/bin/env bash
# Turn raw payload capture on (or off) on the dogfood stack (#1911).
#
# `gateway.toml` declares `[logging.payload_capture] enabled = true`, but that
# copy only governs the gateway until the control plane's first snapshot. From
# then on the `logging_settings` row does, and it ships with capture off. Only
# `rolter-seed --import dogfood.toml` (`just dogfood-seed`) writes it from a
# file, and `just dogfood` deliberately does not seed the fleet, so without this
# a stack filled by hand has an empty Logs drawer a few seconds after boot.
#
#   ./integration/dogfood/payload-capture.sh on    # what `just dogfood` runs
#   ./integration/dogfood/payload-capture.sh off   # compare against capture off
#
# `PUT /api/v1/logging-settings` takes the whole policy, so this reads the
# current one and changes only `payload_capture_enabled`: sampling, the size
# cap, redaction, the model and key filters and retention stay wherever the
# dashboard left them.
#
# Reads ROLTER_CONTROL_URL when set (default http://127.0.0.1:4001) and
# ROLTER_ADMIN_TOKEN from the environment or `.tokens.env`. Waits up to
# CONTROL_WAIT_SECS (default 60) for the control plane to answer, since
# `just dogfood` calls this right after starting it.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
control="${ROLTER_CONTROL_URL:-http://127.0.0.1:4001}"
control="${control%/}"
url="$control/api/v1/logging-settings"

mode="${1:-on}"
case "$mode" in
  on) want=true ;;
  off) want=false ;;
  *)
    echo "usage: $0 [on|off]" >&2
    exit 2
    ;;
esac

if [ -z "${ROLTER_ADMIN_TOKEN:-}" ] && [ -f "$here/.tokens.env" ]; then
  set -a
  # shellcheck source=/dev/null
  . "$here/.tokens.env"
  set +a
fi
auth=()
if [ -n "${ROLTER_ADMIN_TOKEN:-}" ]; then
  auth=(-H "authorization: Bearer $ROLTER_ADMIN_TOKEN")
fi

# only "nothing is listening yet" is worth waiting out; any http answer other
# than 200 (a 401 from a stale token, say) is reported as it is
status=000
for _ in $(seq 1 "${CONTROL_WAIT_SECS:-60}"); do
  current="$(curl -sS --max-time 5 -w '\n%{http_code}' ${auth[@]+"${auth[@]}"} "$url" 2>/dev/null)" || current=$'\n000'
  status="${current##*$'\n'}"
  current="${current%$'\n'*}"
  [ "$status" = 000 ] || break
  sleep 1
done
if [ "$status" = 000 ]; then
  echo "[capture] no answer from $url — is the control plane up? rerun with: just dogfood-capture $mode" >&2
  exit 1
fi
if [ "$status" != 200 ]; then
  echo "[capture] GET $url answered $status: $current" >&2
  exit 1
fi

body="$(printf '%s' "$current" | WANT="$want" python3 -c '
import json, os, sys
settings = json.load(sys.stdin)
settings.pop("updated_at", None)
settings["payload_capture_enabled"] = os.environ["WANT"] == "true"
print(json.dumps(settings))
')"

# -f would hide the body, and a 400 here names the field it refused
resp="$(curl -sS --max-time 10 -w '\n%{http_code}' ${auth[@]+"${auth[@]}"} -X PUT "$url" \
  -H 'content-type: application/json' --data-binary "$body")" || {
  echo "[capture] PUT $url failed" >&2
  exit 1
}
status="${resp##*$'\n'}"
resp="${resp%$'\n'*}"
if [ "$status" != 200 ]; then
  echo "[capture] PUT $url answered $status: $resp" >&2
  exit 1
fi

printf '%s' "$resp" | python3 -c '
import json, sys
settings = json.load(sys.stdin)
state = "on" if settings["payload_capture_enabled"] else "off"
cap = settings["payload_capture_max_bytes"]
print(f"[capture] payload capture {state} (max_bytes={cap})")
'
