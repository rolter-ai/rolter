# The dogfooding fleet

A local rolter with a fleet that looks like a real one, for sitting down as an
*operator* rather than as the person who wrote the screen (#924).

The built-in `fake-llm` model answers "does the gateway work at all". It does
not answer "what is it like to run this": one route with one target exercises
none of the surfaces an operator lives in — provider groups, routing
strategies, per-target health, key scoping, analytics — and every balancing
strategy looks identical when every target answers in the same time.

The harness lives in `integration/dogfood/`. This page is the entry point; the
directory's `README.md` is the detail, including SigNoz and the `.localhost`
names.

## What it stands up

| Piece | Where |
|---|---|
| fifteen fake OpenAI-compatible upstreams on `127.0.0.1:18001-18015` | `integration/dogfood/fleet.ts` |
| the matching rolter config — fifteen providers, three provider groups, eleven routes | `integration/dogfood/dogfood.toml` |
| the keys those upstreams expect (fake, loopback-only, checked in on purpose) | `integration/dogfood/keys.env` |
| the one local login every service shares | `integration/dogfood/creds.env` |
| the SigNoz dashboards the session is read through | `integration/dogfood/signoz/dashboards/` |

Three shapes of upstream, named the way each of them names things:

- `:18001` — OpenAI's model names (`gpt-4o`, `o3-mini`), added as an `openai` provider
- `:18002` — OpenRouter's `vendor/model` names, declared `kind = "openrouter"`
- `:18003-18015` — a self-hosted vLLM/TEI fleet, one model per instance, roughly half behind a key

Three of them exist so failure is legible: `vllm-a100-03` is ~4x slower than its
pair, `vllm-spot-01` returns a 503 for about a quarter of requests, and
`vllm-spot-02` takes ~1.4s to first token. A fleet with no bad targets leaves
the health, breaker and latency screens permanently green, and a permanently
green screen tells an operator nothing.

## Running it

```bash
just dogfood        # datastores, fleet, control, gateway, SigNoz, then the sheet
just dogfood-sheet  # re-print every url, login, endpoint and model
just dogfood-key    # mint another gateway virtual key
just dogfood-seed   # re-import dogfood.toml over a running stack
```

`--import` is desired state, so re-importing an edited `dogfood.toml` updates
the rows it already created rather than duplicating them.

The whole stack binds to loopback, talks only to the fake providers, and holds
nothing real. That is the only reason its credentials are checked in — do not
carry them anywhere else, and leave `docker/docker-compose.yml` and the Helm
chart on their own defaults.

## Things that are load-bearing and easy to miss

- **The stack enforces RBAC, and that is the point.** `just dogfood` generates
  `ROLTER_ADMIN_TOKEN`, `ROLTER_INTERNAL_TOKEN` and both peppers into
  `integration/dogfood/.tokens.env` on first run and serves `/internal/*` on its
  own port, `4002`, as the e2e stack does (#636). RBAC only enforces once the
  control plane has an admin token, so a stack started without one runs with the
  operator API wide open — and then cannot surface the one class of bug
  dogfooding exists to catch: #942, where `/me/*` routes `401`ed while every
  admin route passed, is invisible on a stack that never authenticates anything
  (#1649). A hand-rolled run must set all four. Curl the operator API with
  `-H "authorization: Bearer $ROLTER_ADMIN_TOKEN"`; `just dogfood-sheet` prints
  every value.
- **The two peppers must outlive a restart,** like the KEK. `ROLTER_KEY_PEPPER`
  is mixed into a stored virtual-key digest and `ROLTER_SESSION_PEPPER` into a
  session, so regenerating either invalidates every key and logs everyone out.
  That is why they are written once to a gitignored file rather than being
  checked in beside the other local credentials, and why the gateway is given
  the *same* `ROLTER_KEY_PEPPER` as the control plane — the snapshot carries no
  pepper, so a mismatch rejects every key with `401 invalid api key`.
- **The gateway needs a node id.** Without `ROLTER_NODE_ID` (and without a
  `HOSTNAME`, which a shell-launched process does not have — a container gets
  one from the runtime) the gateway posts its cluster heartbeat and its
  adaptive-routing telemetry with no node header. The control plane drops both
  with a `204`, nothing is logged on either side, and the Cluster and Adaptive
  Routing screens stay empty forever. `just dogfood` sets it; a hand-rolled run
  must too. See #1644.
- **Everything in `dogfood.toml` is in the explicit `readonly` tier.** Providers
  and groups alike are written `[[providers.readonly]]` / `[[provider_groups.readonly]]`
  rather than as the deprecated bare arrays (ADR-0022), because this file is
  what a new operator reads before writing their own (#1657, #1650). The
  `default` tier is not an option for this stack: giving the *control* plane a
  config file switches it to `MergedConfigStore`, which drops db-created virtual
  keys from the snapshot (#623) and then 401s every key the stack mints. An
  unknown tier name is not an error — the config lint reports it as an
  unrecognised key and the entries are silently ignored, so a typo costs the
  whole fleet.
- **Provider groups only propagate through a seed or a restart, and do not
  fan out.** A group created through the dashboard does not bump
  `config_version`, so the gateway never learns about it and `group-slug/model`
  404s until it restarts (#1643). `dogfood.toml` seeds three groups for that
  reason, so group addressing is exercisable today — but every request to a
  group currently lands on its first member whatever the strategy and weights
  say (#1655). Groups in this file are in the explicit `readonly` tier: the
  `default` tier is seeded by the control plane from its own `ROLTER_CONFIG`,
  and giving the control plane a config file switches it to `MergedConfigStore`,
  which drops db-created virtual keys from the snapshot (#623) and 401s every
  key this stack mints.
- **`allow_custom_api_base` does not survive the database.** `dogfood.toml`
  sets it on the OpenRouter-shaped edge, but the column does not exist yet
  (#1133), so once the fleet is seeded the control plane omits that provider
  from the snapshot and the `claude-sonnet-4` route with it. The gateway says
  so on every reload and the dashboard shows it under config problems; it is
  expected until #1133 lands.
- **Analytics need the gateway's `[logging].clickhouse_url`,** not just the
  control plane's `CLICKHOUSE_URL`. The control plane's only lets it *read* the
  table (#929).

## Using it

The point is not that it boots. The point is to do operator work against it —
add a provider, group it, route to it, scope a key, push traffic, then read the
dashboard and the traces back — and to file an issue for every place that goes
badly. A finding mentioned in a summary is lost; a finding on the board is not.
The board conventions are in [issue tracking](issue-tracking.md).

If an operator action produces no useful span in SigNoz, that is an
observability gap and worth its own issue too.
