#!/usr/bin/env bash
# SessionStart hook: start the Storybook dev server on :6006 when nothing is
# listening there. `.mcp.json` points the rolter-storybook MCP server at
# 127.0.0.1:6006/mcp, and that server is the Storybook dev server itself, so
# with it down the MCP tools and the shared story index are gone and someone has
# to restart it by hand.
#
# it returns at once and prints nothing when the port is taken or there is no
# ui/node_modules to run it from. otherwise it starts the server detached, so it
# outlives the session, with its log in .claude/storybook-session.log (git
# ignores *.log). `--exact-port` makes a second start fail in the log instead of
# drifting to a port the MCP config cannot reach.
#
# a session's MCP servers attach while it starts, so the server this starts is
# up for the next session (or after `/mcp` reconnects), not for this one.
#
# ROLTER_STORYBOOK_PORT exists for scripts/test-fleet-scripts.sh;
# leave it unset.
set -u

port=${ROLTER_STORYBOOK_PORT:-6006}
root=${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}
ui="$root/ui"
bin="$ui/node_modules/.bin/storybook"

# a connect to a closed local port is refused at once, so this never waits
if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
  exit 0
fi
[ -x "$bin" ] || exit 0

log="$root/.claude/storybook-session.log"
mkdir -p "$(dirname "$log")"
(
  cd "$ui" || exit 0
  exec nohup "$bin" dev -p "$port" --ci --no-open --exact-port </dev/null >"$log" 2>&1 &
)
exit 0
