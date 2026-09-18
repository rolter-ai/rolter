#!/usr/bin/env bash
# prettier over the markdown, mdx, json and yaml that lives outside ui/.
#
# ui/ runs its own `bun run format:check` against ui/.prettierrc; this is the
# other half of the tree, which had no formatter at all until #1695. the style
# is the root .prettierrc and the exclusions are .prettierignore — never pass
# ad-hoc flags here, or the hook, the ci job and a local run stop agreeing.
#
# usage: scripts/format-docs.sh [--check|--write]   (default: --check)
set -euo pipefail

mode="${1:---check}"
case "${mode}" in
    --check | --write) ;;
    *)
        echo "usage: $0 [--check|--write]" >&2
        exit 2
        ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"

if ! command -v bun >/dev/null 2>&1; then
    echo "bun is required to run prettier; see docs/dev-docs/development/setup.md" >&2
    exit 1
fi

# one prettier version for the whole repository. ui/ pins it as a devDependency
# and bunx resolves the same build here, so a dependabot bump to ui/package.json
# moves both halves of the tree at once and they can never drift into two styles
version="$(python3 -c 'import json,sys; print(json.load(open("ui/package.json"))["devDependencies"]["prettier"])')"

# globs rather than `prettier .`: the root run owns prose and data files, while
# every js/ts/css source in the repo lives under ui/ and is ui/'s to format
exec bunx "prettier@${version}" "${mode}" \
    "**/*.md" \
    "**/*.mdx" \
    "**/*.json" \
    "**/*.yml" \
    "**/*.yaml"
