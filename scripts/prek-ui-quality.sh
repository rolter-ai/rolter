#!/usr/bin/env bash
set -euo pipefail

if ! command -v bun >/dev/null 2>&1; then
    echo "bun is required for the UI pre-push checks" >&2
    exit 1
fi

cd ui
bun run lint
bun run format:check
bun run build
