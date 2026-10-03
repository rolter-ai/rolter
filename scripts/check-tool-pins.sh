#!/usr/bin/env bash
# keeps every `uvx` tool version in .github/tool-pins/requirements.txt (#2185).
# dependabot cannot read a version out of a `uvx tool@x.y.z` command line, so
# the pins live in a requirements file its pip ecosystem does track, and
# everything else reads them through scripts/tool-pin.sh. this fails on
#   - an inline `uvx <tool>@<version>` / `uvx --from <tool>==<version>`
#   - a malformed manifest line (anything but `name==x.y.z`)
#   - a `tool-pin.sh <name>` reference with no manifest entry
#   - a manifest entry nothing references (a stale pin nobody runs)
#
#   scripts/check-tool-pins.sh               # check the tree
#   scripts/check-tool-pins.sh --self-test   # prove each rule can fail
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

check() {
  local root="$1" manifest status=0
  manifest="${root}/.github/tool-pins/requirements.txt"
  if [ ! -f "${manifest}" ]; then
    echo "check-tool-pins: ${manifest} is missing" >&2
    return 1
  fi

  local bad
  bad="$(grep -vE '^(#.*|[[:space:]]*|[A-Za-z0-9_.-]+==[0-9][A-Za-z0-9_.+-]*)$' "${manifest}" || true)"
  if [ -n "${bad}" ]; then
    echo "check-tool-pins: manifest lines must be 'name==x.y.z':" >&2
    echo "${bad}" >&2
    status=1
  fi

  # the places a command line can hide a pin. the guard and its manifest are
  # exempt: they are the only files allowed to spell a version out
  local targets=()
  local t
  for t in .github prek.toml justfile scripts docs/dev-docs/development; do
    [ -e "${root}/${t}" ] && targets+=("${root}/${t}")
  done
  local inline
  inline="$(grep -rnE '(uvx|uv tool run) +(--[a-z-]+ +)*"?[A-Za-z0-9_.-]+(@|==)[0-9]' "${targets[@]}" \
    --exclude=check-tool-pins.sh --exclude=requirements.txt || true)"
  if [ -n "${inline}" ]; then
    echo "check-tool-pins: inline uvx pin; use uvx --from \"\$(bash scripts/tool-pin.sh <tool>)\" <tool> and keep the version in .github/tool-pins/requirements.txt:" >&2
    echo "${inline}" >&2
    status=1
  fi

  local refs names
  refs="$(grep -rhoE 'tool-pin\.sh +[a-z0-9_.-]+' "${targets[@]}" \
    --exclude=check-tool-pins.sh | awk '{print $2}' | sort -u || true)"
  names="$(grep -E '^[A-Za-z0-9_.-]+==' "${manifest}" | sed 's/==.*//' | sort -u || true)"
  local n
  for n in ${refs}; do
    if ! grep -qx "${n}" <<<"${names}"; then
      echo "check-tool-pins: tool-pin.sh ${n} is referenced but has no entry in the manifest" >&2
      status=1
    fi
  done
  for n in ${names}; do
    if ! grep -qx "${n}" <<<"${refs}"; then
      echo "check-tool-pins: ${n} is pinned in the manifest but nothing runs it through tool-pin.sh" >&2
      status=1
    fi
  done
  return "${status}"
}

self_test() {
  local tmp rc=0
  tmp="$(mktemp -d)"
  trap 'rm -rf "${tmp}"' RETURN
  expect_fail() {
    local label="$1"
    if check "${tmp}/t" >/dev/null 2>&1; then
      echo "self-test: '${label}' was not caught" >&2
      rc=1
    else
      echo "self-test: '${label}' caught"
    fi
  }
  fresh() {
    rm -rf "${tmp}/t"
    mkdir -p "${tmp}/t/.github/workflows" "${tmp}/t/.github/tool-pins" "${tmp}/t/scripts"
    printf 'zizmor==1.0.0\n' >"${tmp}/t/.github/tool-pins/requirements.txt"
    printf '%s\n' 'run: uvx --from "$(bash scripts/tool-pin.sh zizmor)" zizmor' >"${tmp}/t/.github/workflows/a.yml"
  }
  fresh
  check "${tmp}/t" || { echo "self-test: clean fixture rejected" >&2; rc=1; }
  fresh; echo 'run: uvx zizmor@1.0.0 .' >>"${tmp}/t/.github/workflows/a.yml"; expect_fail "inline @ pin"
  fresh; echo 'run: uvx --from zizmor==1.0.0 zizmor' >>"${tmp}/t/.github/workflows/a.yml"; expect_fail "inline == pin"
  fresh; echo 'zizmor>=1' >>"${tmp}/t/.github/tool-pins/requirements.txt"; expect_fail "unpinned manifest line"
  fresh; echo 'run: uvx --from "$(bash scripts/tool-pin.sh ruff)" ruff' >>"${tmp}/t/.github/workflows/a.yml"; expect_fail "unknown tool"
  fresh; echo 'ruff==0.1.0' >>"${tmp}/t/.github/tool-pins/requirements.txt"; expect_fail "unused manifest entry"
  return "${rc}"
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
else
  check "${1:-${here}}"
fi
