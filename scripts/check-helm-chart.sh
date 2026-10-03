#!/usr/bin/env bash
# render charts/rolter and check the output strictly. the one definition of the
# chart's render cases: quality.yml runs each case as its own step, and the
# `helm-render` prek hook runs `all`, so a chart edit fails locally with the
# message CI would print (#2191).
#
#   bash scripts/check-helm-chart.sh [all|defaults|no-preflight|all-preflight|update-check]
#
# `all` (the default) also runs `helm lint`. without helm on PATH `all` skips
# with a notice, since CI is the authority; naming a case needs helm and fails.
set -euo pipefail

cd "$(dirname "$0")/.."

chart=charts/rolter
strict="uv run --script scripts/check-rendered-manifests.py"
update_check="uv run --script scripts/check-chart-update-check.py"

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
        ;;
    defaults) render_defaults ;;
    no-preflight) render_no_preflight ;;
    all-preflight) render_all_preflight ;;
    update-check) render_update_check ;;
    *)
        echo "unknown case: $case_name" >&2
        exit 2
        ;;
esac
