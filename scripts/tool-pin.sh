#!/usr/bin/env bash
# prints the pinned requirement for a uvx tool, e.g. `zizmor==1.26.1`, from
# .github/tool-pins/requirements.txt. use it as
#   uvx --from "$(bash scripts/tool-pin.sh zizmor)" zizmor ...
# so the version lives in one file dependabot tracks (#2185)
set -euo pipefail

root="${TOOL_PINS_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
file="${root}/.github/tool-pins/requirements.txt"

if [ "$#" -ne 1 ]; then
  echo "usage: tool-pin.sh <tool>" >&2
  exit 2
fi

pin="$(grep -E "^${1}==[0-9]" "${file}" || true)"
if [ -z "${pin}" ]; then
  echo "tool-pin.sh: no pin for '${1}' in ${file}" >&2
  exit 1
fi
printf '%s\n' "${pin}"
