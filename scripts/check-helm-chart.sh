#!/usr/bin/env bash
# render charts/rolter and check the output strictly. the one definition of the
# chart's render cases: quality.yml runs each case as its own step, and the
# `helm-render` prek hook runs `all`, so a chart edit fails locally with the
# message CI would print (#2191).
#
#   bash scripts/check-helm-chart.sh [all|defaults|no-preflight|all-preflight|update-check|image-tags|preflight]
#
# `all` (the default) also runs `helm lint`. without helm on PATH `all` skips
# with a notice, since CI is the authority; naming a case needs helm and fails.
set -euo pipefail

cd "$(dirname "$0")/.."

chart=charts/rolter
strict="uv run --script scripts/check-rendered-manifests.py"
update_check="uv run --script scripts/check-chart-update-check.py"
image_tags="uv run --script scripts/check-chart-image-tags.py"

render_defaults() {
    helm template ci "$chart" | $strict
}

render_no_preflight() {
    helm template ci "$chart" --set preflight.enabled=false | $strict
}

render_all_preflight() {
    helm template ci "$chart" \
        --set preflight.connect=true \
        --set env.databaseUrl=postgres://u:p@db:5432/rolter \
        --set 'secretEnv[0].name=ROLTER_KEK' \
        --set 'secretEnv[0].valueFrom.secretKeyRef.name=rolter' \
        --set 'secretEnv[0].valueFrom.secretKeyRef.key=kek' \
        | $strict
}

# the launcher in each Deployment's preflight initContainer runs its own
# release check, so one setting must reach both (#2382)
render_update_check() {
    helm template ci "$chart" | $update_check true
    helm template ci "$chart" --set control.updateCheck=false | $update_check false
}

# the control plane and the gateways are tagged apart (#3352): by default both
# run `image.tag`, or the appVersion when that is empty, and a component's own
# tag moves that component alone, preflight initContainer included
render_image_tags() {
    local app
    app="$(sed -n 's/^appVersion: *"\{0,1\}\([^"]*\)"\{0,1\}$/\1/p' "$chart/Chart.yaml")"
    helm template ci "$chart" | $image_tags "$app" "$app"
    helm template ci "$chart" --set image.tag=9.9.9 | $image_tags 9.9.9 9.9.9
    helm template ci "$chart" --set image.tag=9.9.9 --set gateway.image.tag=9.8.0 \
        | $image_tags 9.9.9 9.8.0
    helm template ci "$chart" --set control.image.tag=9.9.9 | $image_tags 9.9.9 "$app"
}

# the default values plus the secrets and datastores an operator must supply:
# the real `rolter check --strict` of each preflight init container has to pass
# on its own rendered env, including the chart's 0.0.0.0 bind (#1939). needs a
# built binary, so `all` leaves it out unless ROLTER_BIN is set
render_preflight() {
    local bin="${ROLTER_BIN:-target/debug/rolter}"
    if [ ! -x "$bin" ]; then
        echo "no rolter binary at $bin: build it (cargo build -p rolter) or set ROLTER_BIN" >&2
        exit 1
    fi
    helm template ci "$chart" \
        --set env.databaseUrl=postgres://u:p@db:5432/rolter \
        --set env.redisUrl=redis://redis:6379 \
        --set env.clickhouseUrl=http://clickhouse:8123 \
        --set 'secretEnv[0].name=ROLTER_KEK' \
        --set 'secretEnv[0].value=0123456789abcdef0123456789abcdef' \
        --set 'secretEnv[1].name=ROLTER_ADMIN_TOKEN' \
        --set 'secretEnv[1].value=an-admin-token-for-the-chart-test' \
        --set 'secretEnv[2].name=ROLTER_KEY_PEPPER' \
        --set 'secretEnv[2].value=a-deployment-wide-pepper' \
        --set 'secretEnv[3].name=ROLTER_SESSION_PEPPER' \
        --set 'secretEnv[3].value=a-control-plane-session-pepper' \
        | uv run --script scripts/check-chart-preflight.py "$bin"
}

case_name="${1:-all}"

if [ "$case_name" = all ] && ! command -v helm >/dev/null 2>&1; then
    echo "helm is not on PATH: skipping the chart render (CI still runs it). install helm to check locally" >&2
    exit 0
fi
if ! command -v helm >/dev/null 2>&1; then
    echo "helm is required to render the chart" >&2
    exit 1
fi
if ! command -v uv >/dev/null 2>&1; then
    echo "uv is required to run the strict manifest checker" >&2
    exit 1
fi

case "$case_name" in
    all)
        helm lint "$chart"
        render_defaults
        render_no_preflight
        render_all_preflight
        render_update_check
        render_image_tags
        if [ -n "${ROLTER_BIN:-}" ]; then render_preflight; fi
        ;;
    defaults) render_defaults ;;
    no-preflight) render_no_preflight ;;
    all-preflight) render_all_preflight ;;
    update-check) render_update_check ;;
    image-tags) render_image_tags ;;
    preflight) render_preflight ;;
    *)
        echo "unknown case: $case_name" >&2
        exit 2
        ;;
esac
