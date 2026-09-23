#!/usr/bin/env bash
# Summarise what the dogfooding operator has been doing and hitting (#924).
#
# Reads the two ClickHouse tables a dogfood session fills — `ui_events` (the
# dashboard's own UX stream, #805) and `request_logs` (the gateway) — and
# prints the signals worth reacting to while the session is still going:
# where time went, which forms were abandoned or retried, which controls were
# refused, which screens errored, and what the gateway returned.
#
#   ./integration/dogfood/watch.sh            # the last 30 minutes
#   ./integration/dogfood/watch.sh 240        # the last 4 hours
#   WATCH_EVERY=60 ./integration/dogfood/watch.sh 30   # refresh every minute
#
# Read-only. CLICKHOUSE_URL overrides the loopback default.
set -uo pipefail

CH="${CLICKHOUSE_URL:-http://127.0.0.1:8123}"
MIN="${1:-30}"
case "$MIN" in '' | *[!0-9]*) echo "usage: $0 [minutes]" >&2; exit 2 ;; esac

q() {
  curl -fsS --max-time 10 "$CH/?default_format=PrettyCompactMonoBlock" --data-binary "$1" 2>&1 \
    || echo "  (query failed — is ClickHouse up at $CH?)"
}
bold() { printf '\n\033[1m%s\033[0m\n' "$1"; }

report() {
  local w="ts > now() - interval $MIN minute"
  printf '\033[2m%s · last %s min · %s\033[0m\n' "$(date '+%H:%M:%S')" "$MIN" "$CH"

  bold "Dashboard · activity by screen"
  q "select screen,
       countIf(action = 'screen_view') as views,
       round(quantileIf(0.5)(duration_ms, action = 'time_to_interactive')) as tti_p50_ms,
       round(quantileIf(0.95)(duration_ms, action = 'time_to_interactive')) as tti_p95_ms,
       countIf(action = 'form_submit') as submits,
       countIf(action = 'form_abandon') as abandons,
       countIf(action = 'error_state') as errors,
       countIf(action = 'empty_state') as empties
     from ui_events where $w group by screen order by views desc limit 25"

  bold "Dashboard · struggle signals"
  q "select action, screen, target, outcome, count() as n, max(ts) as last
     from ui_events
     where $w and action in ('form_abandon', 'validation_error', 'error_state',
                             'retry_submit', 'refused_click', 'abandon_dirty', 'back_out')
        or ($w and outcome = 'error')
     group by action, screen, target, outcome order by n desc, last desc limit 25"

  bold "Dashboard · last 15 events"
  q "select formatDateTime(ts, '%H:%i:%S') as t, screen, action, target, outcome, duration_ms
     from ui_events where $w order by ts desc limit 15"

  bold "Gateway · by model and status"
  q "select model, provider, status, count() as n,
       round(quantile(0.5)(latency_ms)) as p50_ms, round(quantile(0.95)(latency_ms)) as p95_ms,
       round(quantile(0.5)(ttft_ms)) as ttft_p50, sum(total_tokens) as tokens,
       round(sum(cost_usd), 4) as cost
     from request_logs where $w group by model, provider, status order by n desc limit 25"

  bold "Gateway · last errors"
  q "select formatDateTime(ts, '%H:%i:%S') as t, model, provider, status, substring(error, 1, 90) as error
     from request_logs where $w and status >= 400 order by ts desc limit 10"
}

if [ -n "${WATCH_EVERY:-}" ]; then
  while true; do
    clear
    report
    sleep "$WATCH_EVERY"
  done
else
  report
fi
