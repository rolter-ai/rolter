# Security

## Secret handling

- **Upstream provider keys** are never stored in plaintext in the database. They are **envelope-encrypted** with AES-256-GCM: a per-record data key/nonce, wrapped by a master key (KEK) supplied via `ROLTER_KEK` (env/file). Pluggable backends (HashiCorp Vault, cloud KMS) are a roadmap item.
- In the **bootstrap file**, prefer `api_key_env` over inline `api_key` so secrets stay in the environment, not on disk.
- **Virtual keys** are stored as hashes with a short display prefix; the raw key is shown once at creation.
- Secrets are never logged. The gateway redacts auth headers from traces.

## Transport

- Upstream calls use rustls (no OpenSSL). HTTP/2 keep-alive with connection pooling.
- Optional per-provider **egress proxy** (`egress_proxy`, HTTP/HTTPS/SOCKS5) for networks where providers aren't directly reachable.
- Optional global or per-provider **custom CA bundles** add private PKI roots to outbound upstream clients while retaining public roots, certificate-chain validation, and hostname verification.
- Terminate TLS at the gateway or a fronting proxy/ingress in production.

## Cross-origin policy (CORS)

`security_settings.allowed_origins` and `allowed_headers` are enforced by a
middleware on the **control plane** (`crates/rolter-control/src/cors.rs`). The
policy is read from an `ArcSwap` per request, so an edit through
`PUT /api/v1/security-settings` applies immediately rather than at the next
restart.

Only the control plane has one, because it is the only origin a browser talks
to: it serves the dashboard, and the Playground reaches the data plane through
the `/gw/*` reverse proxy rather than calling the gateway directly. A gateway
CORS layer would govern requests browsers do not make, and would need
`SecuritySettings` in the snapshot to do it.

Behaviour:

- **No origins configured is the default**, and the same-origin deployment
  everyone runs. The middleware adds no headers at all and the request path is
  unchanged — CORS is not needed when the control plane serves both the
  dashboard and the API.
- Matching is **exact** against the configured list, modulo case and a trailing
  slash. There is no wildcard and no suffix match, so `evil-dash.example.com`
  and `dash.example.com.evil.test` do not inherit an entry for
  `dash.example.com`. `validate_origin` already rejects `*` at write time.
- A denied origin gets a normal response with no CORS headers; the browser
  enforces the block. `Vary: Origin` is set on every answer so a shared cache
  cannot serve one origin's response to another.
- Preflights are answered by the middleware with `204` and never reach the
  router, since they carry no credentials.
- `authorization`, `content-type`, `traceparent`, `tracestate` and
  `x-request-id` are **always** allowed, unioned with whatever is configured.
  The trace headers are what let the dashboard's spans join the gateway's on a
  split-origin deployment; leaving them to configuration means browser trace
  propagation silently does not work until someone remembers them.
- `x-request-id` is exposed via `Access-Control-Expose-Headers`, so a browser
  can read the correlation id the API already echoes.

## Gateway ingress policy (#1162)

The rest of `security_settings` — the part that is not CORS — reaches the
gateway through `/internal/snapshot` as `GatewayConfig::security`
(`rolter_core::SecurityPolicyConfig`), loaded by
`PostgresConfigStore::load_security_policy`. The table's own trigger bumps
`config_version`, so an edit propagates on the next poll like any other config
change, with no restart.

Only the **policy** columns are selected. The dashboard credential ciphertext
and nonce are not named by the query at all — the migration promised snapshots
would never carry them, and a query that cannot see a column cannot leak it.

| Setting                | Effect on the gateway                                                                       |
| ---------------------- | ------------------------------------------------------------------------------------------- |
| `virtual_key_required` | an unauthenticated request is refused even where the gateway holds no keys                  |
| `required_headers`     | a request missing any name/value pair is refused at ingress, before routing and before auth |
| `auth_bypass_routes`   | the named paths answer without a key                                                        |

Load-bearing properties, each with a test that fails if it stops holding:

- **Every rule can only close, never open** — except `auth_bypass_routes`,
  which an operator writes out path by path. The default for the whole struct
  is "no extra rules", so a store that cannot be read leaves the deployment
  where it was rather than opening one that was closing.
- **`server.require_auth` in the config file still wins, in both directions.**
  A file is a deliberate local override by whoever runs the process, who may
  not be the person holding the dashboard.
- **Bypass matching is exact.** `/v1/models` does not open
  `/v1/models/gpt-4o`, and `validate_bypass_route` already refuses wildcards
  and non-`/v1` paths at write time. The MCP surface passes a literal `/mcp`
  into `authenticate` rather than its real path, so no bypass entry can ever
  name it.
- **`/healthz` and `/readyz` are exempt from `required_headers`.** A probe is
  the deployment's own traffic; filtering it takes the gateway down rather than
  protecting it.
- **A refusal names the header and never its value.** An operator may well have
  put a shared secret in a required header, and echoing it would hand it to the
  one caller who did not know it.

`allow_direct_provider_keys` is **gone from the API and the dashboard.** The
gateway has no direct-provider-key passthrough, so the column controlled
nothing; the store now pins it to its default. A toggle that reads like a
security control and does nothing is worse than an absent one, because it
converts into a false belief during exactly the review where it matters.

## The Security screen (#2103, #2114)

The dashboard's Security screen (`ui/src/pages/Security.tsx`) saves every field in one
`PUT /api/v1/security-settings`, and the control plane refuses the whole save when one entry breaks a
rule in `validate_settings`. Three pieces of the screen exist to keep that refusal from being the
first the operator hears of it.

- **The list rules are mirrored in `ui/src/lib/security-lists.ts`.** Every list is one entry per
  line, and an entry that does not parse is kept in the field and named with its line, never
  dropped. The functions accept what `validate_settings` accepts and refuse what it refuses, no
  stricter and no looser. The one extra rule is a required header named twice, since the store
  keeps the pairs as a map and one of the two values would vanish. `security-lists.test.ts` reads
  `security.rs` and fails when the forbidden-character sets or the `/v1/` prefix change there, so a
  rule widened on one side shows up as a red test rather than as a toast.
- **`ui/src/lib/security-loosening.ts` decides which saves ask first.** Only three edits loosen:
  `virtual_key_required` going on to off, `dashboard_auth_enabled` going on to off, and a path added
  to `auth_bypass_routes`. It compares the draft with what the store held at the last load or save,
  and a save that only tightens goes out without a dialog (see
  [destructive actions](../development/destructive-actions.md)).
- **`ui/src/lib/gateway-pickup.ts` reads the fleet after a save.** The write bumps `config_version`
  (the table's trigger), each gateway reports the version it runs on its snapshot poll, and
  `GET /api/v1/cluster/nodes` compares the two as `converged`. The screen asks for the inventory
  under a key of its own per save, so an answer read before the save is never taken for one after it,
  and counts the live gateways that converged. With no live gateway on record, or a failed read, it
  says pickup cannot be confirmed and when a gateway applies it (next poll, `ROLTER_SNAPSHOT_POLL_SECS`,
  5 s by default; at once with Redis pub/sub).

The virtual-key switch is narrower than its label in the gateway: `authenticate` in
`crates/rolter-gateway/src/handlers.rs` reads `virtual_key_required || managed_auth` for a gateway with
an empty key set, and every gateway that receives the setting through a snapshot is managed, so the
switch never changes what one of them does (#2357). The confirmation words the consequence as the
documented one (the gateway "decides by how it was started") and claims no more.
`dashboard_auth_enabled` and the dashboard credential are stored and returned, and no code path in the
control plane reads them yet (#2356), so the dashboard-protection row says what the setting asks for
and nothing about an effect.

## Egress policy (SSRF)

A provider's `api_base` decides where the gateway sends traffic, so an admin
surface that accepts an arbitrary one turns the proxy into an SSRF primitive
aimed at whatever the gateway's network position can reach.

rolter is built to run self-hosted and air-gapped, so upstreams legitimately
live on loopback, RFC1918 and container networks — blanket-denying private
destinations would break the core use case. What no legitimate LLM upstream
needs is the **link-local** range, which is where cloud instance metadata lives
(`169.254.169.254`, `fe80::/10`). That is denied by default; everything else is
opt-in:

```toml
[egress]
block_link_local = true   # default — cloud instance metadata
block_loopback   = false  # sidecar / single-host deployments
block_private    = false  # on-prem clusters
allow_hosts      = []     # exact-host escape hatch
```

Enforced in two places: the control plane rejects a denied `api_base` at
**write time** (`400`, so it never reaches the database), and snapshot
validation re-checks it, so a bootstrap toml can't smuggle one in either.

Config-time validation classifies **IP literals** only. Resolving hostnames
there would make validation depend on live DNS — reintroducing the "one bad row
freezes every gateway" failure mode described in
[config-and-hot-reload.md](config-and-hot-reload.md) — and would be bypassable
by DNS rebinding regardless.

A literal is read the way the HTTP client reads it. `EgressPolicy::check_url`
parses with the WHATWG `url` crate that `reqwest` uses, because that parser
normalizes the host before anything connects: `http://2852039166/`,
`http://0xa9fea9fe/`, `http://169.254.43518/`, `http://0251.0376.0251.0376/`
and `http://169.254.169.254./` all dial `169.254.169.254`, and a backslash ends
the authority, so the host of `http://169.254.169.254\@example.com/` is the
address. A hand-rolled host split saw none of those as that address, and
`hyper` never calls a resolver for an IP literal, so the connect-time check
below did not see them either. `deny_reason` also classifies an IPv4-mapped
IPv6 address (`::ffff:169.254.169.254`) as the IPv4 address it carries and
treats the unspecified address (`0.0.0.0`, `::`) as loopback, since connecting
to it reaches the local host. A socks proxy URL keeps its host opaque to the
parser, so that host is parsed again as an `http` host would be. `allow_hosts`
entries are parsed the same way and match the address, so an entry for
`169.254.169.254` also covers `0xa9fea9fe`.

So the same policy is enforced a third time, at **connect time**, by a custom
resolver on every upstream client: whatever DNS actually returns is classified
immediately before the connection is made. That covers the two cases config
validation cannot see — a hostname like `metadata.internal`, and a name that
resolved to a public address when it was configured but resolves to
`169.254.169.254` at request time. A denied destination surfaces as a policy
error naming the host, not an opaque connect failure.

A name that resolves to several addresses keeps the permitted ones: refusing
the whole name would take down a legitimate multi-homed upstream. Only a name
left with nothing is refused — which is exactly what rebinding to a denied
address produces. The resolver reads the policy from a live handle, so a hot
reload re-tunes enforcement without discarding pooled connections.

The control plane enforces the policy at connect time too. Its own requests to
operator-supplied URLs (the connector test probe, alert channel delivery, and
MCP OAuth discovery and token exchange) check the stored URL when it is saved
and again before each request, and every one of them is sent through the client
built by `crates/rolter-control/src/egress_client.rs`. That builder installs a
resolver that drops the addresses `EgressPolicy::filter_resolved` denies, the
same function the gateway's `EgressResolver` calls, so the two planes cannot
disagree about an address. It also refuses redirects, so a `3xx` cannot hand a
request to a host the check never saw; the connector probe used to send through
the shared `ControlState::http` client, which follows up to ten. The client is
built per request from the deployment's policy rather than pooled, since these
calls are rare.

## Control-plane input validation

Every control-plane mutation body is decoded through a `SafeJson` extractor
rather than axum's `Json`. Before the body is deserialized into its typed
struct, every string in it — nested objects, arrays and object keys included —
is screened for control characters, and the request is rejected with a `400`
naming the offending field.

The concrete failure this prevents: Postgres `text` columns cannot store a NUL
byte, so a field carrying one failed deep inside the store and surfaced as an
unhandled `500` instead of input validation, violating the "bounded error, no
`unwrap`/`expect` on a request path" invariant. Screening the whole C0/C1 range
(and `U+007F`) also closes the log-injection vector a raw escape or newline
would otherwise open in operator-facing logs.

Tab, newline and carriage return stay allowed — multi-line values are
legitimate, a PEM CA bundle being the obvious one. Malformed JSON now also
comes back in the same OpenAI-style error envelope as every other failure
instead of axum's default rejection body.

## What a 500 says (#2268)

A control-plane `500` answers `{"error": {"message": "internal server error"}}`
and logs its cause at `error`, unless the message was written for the caller on
purpose. Most server-side failures reach the response as `rolter_core::Error`,
and `Error::Store` carries whatever the driver said: every
`map_err(|e| Error::Store(e.to_string()))` and the store's own `store_err` pass
sqlx text on, which can name relations, schemas, hosts and the credentials in a
connection URL.

A message that is safe and useful to show goes through `ApiError::Curated`
instead (`crates/rolter-control/src/crud.rs`), and is rendered verbatim:

| Message                                               | Where                                            |
| ----------------------------------------------------- | ------------------------------------------------ |
| `INSERT_FAILED`, the "requires CLICKHOUSE_URL" family | `ingest_failure.rs` (the store's text is logged) |
| the reason an alert rule's signal read failed         | `alerting.rs`, worded by `read_signal`           |
| failed to encrypt a connector or channel credential   | `connectors.rs`, `alerting.rs`                   |
| failed to query observability connectors              | `collector_config.rs`                            |
| unknown provider kind in a stored row                 | `crud.rs`                                        |

`4xx` answers are untouched: validation (`Error::Config`) and lookups
(`Error::NotFound`) are written for the caller. A new message that has to reach
a `500` body is a `Curated` one, and must never interpolate anything a driver or
upstream returned.

## Open mode (no admin token)

With no `ROLTER_ADMIN_TOKEN` set, `Principal` short-circuits to `Superadmin` for
every request: the management API and `/internal/snapshot` have no
authentication step to fail. This is the zero-credential local-dev shape, and
`crates/rolter-control/src/open_mode.rs` is what keeps it from being anything
else. Before either listener is opened, it evaluates "is a token set" against
every address about to be bound:

| admin token | bind                               | outcome                                     |
| ----------- | ---------------------------------- | ------------------------------------------- |
| set         | any                                | `Closed` — RBAC enforced                    |
| unset       | all listeners loopback             | `OpenLoopback` — allowed, warned            |
| unset       | any non-loopback listener          | **refuses to start**                        |
| unset       | non-loopback + `--allow-open-mode` | `OpenAcknowledged` — allowed, warned loudly |

`--internal-addr` counts as a listener here: an exposed credential channel is
no better than an exposed API, so either one alone is enough to refuse.

The decision also rides into the dashboard through `window.__ROLTER_CONFIG__`
(`openMode: true`), which renders a persistent banner. Without it the dashboard
looks identical whether the control plane is gated or wide open, which is the
property that made this dangerous rather than merely permissive (#970).

This is why `ROLTER_CONTROL_HOST` defaults to `127.0.0.1` rather than
`0.0.0.0`: containers and clusters set it explicitly, and by then they have a
reason to have set a token too.

The published image is one of those containers. Its default command runs
`rolter easy-up --host 0.0.0.0`, since a published port cannot reach a
container's loopback (#1891), and leaves the refusal above in charge: a bare
`docker run` of the image with no token exits rather than serving. `easy-up`
applies the same rule before it writes, seeds or binds anything, to the host
its control plane binds (`--control-host`/`ROLTER_CONTROL_HOST`, else
`--host`/`ROLTER_HOST`) and, in database mode, to the gateway's host as well,
because the gateway then serves the management API at `/admin/*` by proxying
to the co-hosted control plane. The bind is a flag on that command, not an
image-wide variable, so a `rolter-gateway` or `rolter-control` run from the
image keeps its own default.

## Who reads the request log (#1820)

`/api/v1/analytics/*` (the request log, usage, spend and attribution rollups)
and `/api/v1/health/*` (provider uptime, MTTR, the failure timeline) are merged
onto the part of the router that also serves the probes and `/api/v1/config`,
and until #1820 that made them open: neither resolved a principal, so a caller
with no credentials read every tenant's request logs — captured prompt and
completion bodies included — on a deployment whose CRUD API was enforcing RBAC.

Every handler in both modules now takes an `AnalyticsAccess`
(`crates/rolter-control/src/analytics_access.rs`). It authenticates the way the
CRUD API does and turns what the caller holds into a filter each query binds as
ClickHouse parameters — never spliced SQL:

| caller                                                         | request-log rows                                           | captured bodies                                                                | provider health                                               |
| -------------------------------------------------------------- | ---------------------------------------------------------- | ------------------------------------------------------------------------------ | ------------------------------------------------------------- |
| admin token, superadmin session, open mode                     | all, including rows logged with no org                     | all                                                                            | all                                                           |
| a user with memberships or custom roles                        | rows whose org, team or project any of their roles reaches | where their role at the row's scope meets the `request_payload` floor (member) | provider names used in orgs where they hold an org-level role |
| the same user, on a project set to `payload_min_role = viewer` | unchanged                                                  | also where they hold only viewer there                                         | unchanged                                                     |
| no credentials, or a bearer that is neither token nor session  | `401`                                                      | `401`                                                                          | `401`                                                         |

Two details carry the design:

- **Rows are filtered and bodies are masked in the database.** Viewer is the
  lowest role, so a row is visible when _any_ role reaches it. A body needs the
  role resolved most-specific-first exactly as `rbac::resolve_role` does — an
  org admin who is a viewer on one project reads that project's rows without
  its bodies — and raised by custom roles as `rbac::custom_base_role` does. A
  withheld body comes back empty with `payload_withheld = 1`, so the dashboard
  says "hidden for your role" instead of "payload capture is off".
- **The floors live in the capability matrix.** `analytics:read` (viewer),
  `request_payload:read` (member) and `provider_health:read` (viewer) are rows in
  `CAPABILITIES`, and the extractor reads its thresholds from them through
  `cap!`, so `GET /api/v1/rbac/matrix` publishes the rule the filter applies.
  The per-project exception is `project_settings:update` (admin), stored in
  `projects.payload_min_role`; `GET /api/v1/rbac/effective` folds it in when the
  scope it is asked about names a project and the caller holds a role on that
  project's own chain, read from the project row rather than from the query
  string, so naming another org's project beside one's own org reads nothing
  back.

A captured body is joined to its log row on `(request_id, ts)`, never on the
id alone. The gateway keeps whatever `x-request-id` the caller sent, so two
tenants' requests can share an id; an id-only join let a caller log a bodiless
request under an id seen on another project's rows and read that project's
prompt through their own row, which the mask passed because it judges the row,
not the body. `PayloadLog` copies its log row's `ts` through the same
serializer, so the pair names one request.

The pair is not a perfect key. Two requests that share an `x-request-id` and
were logged in the same millisecond (`ts` is `DateTime64(3)`) still get one
body between them, which takes a client sending predictable or constant ids.
Keying the join on something the caller cannot choose is #1937. The pair also
drops bodies written by gateways from v0.1.0 or earlier, which let ClickHouse
stamp the log row and the payload row separately: after the control plane is
upgraded those bodies stop showing until payload retention removes them. The
user docs' upgrade page says to upgrade gateways first for that reason.

Two limits are known and tracked:

- **Provider health matches by name.** `provider_health_events` carries the
  provider's display name and no org, and names are unique per org only. A
  name that another org used and then deleted brings its history (uptime,
  latency, error kinds, `target_id`) to whichever org creates it next, until
  the 90-day TTL drops it. Provider names are unique across the deployment,
  so two orgs never hold one at the same time. Recording the org in the rows
  is #1908.
- **No database, no dashboard.** A control plane with `ROLTER_ADMIN_TOKEN` and
  no store has no sessions, so the admin token is the only credential these
  routes accept, and the dashboard's e-mail-only sign-in cannot present it. The
  routes stay closed on purpose; letting the dashboard take the token is
  #1909.

What keeps this from regressing is
`every_route_the_spec_does_not_mark_public_refuses_an_anonymous_caller` in
`crates/rolter-control/tests/control_integration.rs`: it walks the served
`/openapi.json` with no credentials and a forged bearer and requires a `401`
from every GET the document does not mark public. The routes that are open by
design (`/api/v1/ping`, `/roles`, `/provider-kinds`, `/currency`, `/config`,
`/config/problems`) are marked `.public()` in `openapi.rs` for that reason, and
`crates/rolter-control/tests/analytics_scoping.rs` pins the row and body rules
against a real ClickHouse.

## Who reads the MCP tool-call log (#1831)

`GET /api/v1/mcp/logs`, `/summary` and `/{event_id}` were superadmin-only, which
left the engineer whose agent's tool call failed with no way to read the row that
said why. They now follow the request-log model above, through the same
`AnalyticsAccess` filter (`McpLogAccess` in `analytics_access.rs` is the extractor
for this table), so there is one implementation of "which rows may this caller
read" rather than two:

| caller                                     | rows                                                                                                | tool arguments and results                                                                                                                |
| ------------------------------------------ | --------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| admin token, superadmin session, open mode | all, including rows with no tenancy                                                                 | all                                                                                                                                       |
| a user with memberships or custom roles    | rows whose org, team or project any of their roles reaches, and every row whose `user_id` is theirs | where their role at the row's scope meets the `request_payload` floor (member), or a viewer on a project with `payload_min_role = viewer` |
| no role anywhere                           | none: an empty list, a summary of zero, `404` on a detail                                           | none                                                                                                                                      |
| no credentials or a forged bearer          | `401`                                                                                               | `401`                                                                                                                                     |

- **The floors are matrix rows.** `mcp_log:read` is `viewer` at project scope
  and the body floor is the existing `request_payload:read`; `mcp_log:create`
  stays superadmin, so only the gateway's credential or a superadmin records an
  event. A custom role may now grant `mcp_log:read`.
- **Own rows.** `mcp_tool_call_logs.user_id` carries the id of the user who owns
  the MCP OAuth session that made the call. The predicate is
  `user_id = caller`, bound as a parameter, and it admits the row without any
  role, so the caller sees their own failures even on a call the gateway could
  not attribute to a project. Their arguments and results still need the body
  floor, which a row with no scope never meets, so an unattributed row's bodies
  stay withheld from its owner.
- **Bodies are masked in the database.** The detail query blanks `arguments` and
  `result` and sets `payload_withheld` for a caller below the floor, like the
  invocation list. The flag reads the table-qualified columns; unqualified it
  would resolve to the masked alias and never fire. A row the caller may not see
  is a `404`, so an event id cannot be probed across tenants.
- **Rows from before attribution.** The table already had `org_id`, `team_id`,
  `project_id` and `user_id`, filled by whoever submits the event, so no
  migration was needed. A row written with those empty (everything the table
  holds from before the gateway attributes its calls) has no tenant and no
  owner, so it stays visible to superadmins and the admin token only.

`crates/rolter-control/tests/mcp_log_scoping.rs` pins the rules against a real
ClickHouse, and `every_route_the_spec_does_not_mark_public_refuses_an_anonymous_caller`
still covers the three routes.

## One org never reaches another (#1844, #1845)

A provider's credential belongs to the org that stored it, and until #1844 the
gateway did not know that: the snapshot carried routes, providers and provider
groups with no owner, so a key from any org could call another org's routes,
or address its providers directly as `provider-slug/model` or through a group
as `group-slug/model`, spending that org's credential. Two orgs that picked the
same provider name also froze config propagation for everyone, because the
snapshot refuses duplicate names.

Every row loaded from the store now carries its org (and a route its project)
as `tenancy`, and the gateway resolves a model only among the rows the key's
org may use, on all three address forms and in `GET /v1/models`. Another org's
row is a miss: it shadows nothing of the caller's own, and the caller gets the
`404` an unknown model gets. On the write path a route target or group member
must name a provider in the same org. Names the gateway indexes
deployment-wide (route and provider names, and the one slug namespace
providers and groups share) are unique across orgs, and so is a route name
that would sit on another org's `slug/model` address. A collision is refused
with a `409` that does not say which org holds the name. The route-name check
runs one way: a slug created after another org's `slug/…` route is accepted,
and keys that carry no org then reach that route rather than the new address
(the ADR-0017 addendum explains why). An admin can also
narrow a route to its own project (`project_only`), which narrows that route
but not the `slug/model` address of the provider behind it (#1919).
The contract, the table of which keys admit which rows, and the write-time
guards are in
[RBAC & authentication](rbac-and-auth.md#one-org-never-reaches-another-1844-1845);
per-org namespaces, which would let two orgs reuse a name, are #1857.

## Control↔data-plane trust boundary

`GET /internal/snapshot` returns provider `api_key`s **decrypted**. That is by
necessity — the data plane needs the upstream credential to authenticate to the
provider — but it makes the snapshot channel a different trust boundary from
the operator-facing management API, and it should be configured as one:

```bash
ROLTER_INTERNAL_TOKEN=...            # gates /internal/*, distinct from ROLTER_ADMIN_TOKEN
ROLTER_INTERNAL_ADDR=127.0.0.1:4002  # serves /internal/* on its own socket
```

With `ROLTER_INTERNAL_TOKEN` set, the operator admin token no longer opens the
snapshot — only the gateway's own credential does. With `ROLTER_INTERNAL_ADDR`
set, `/internal/*` is not mounted on the public API router at all, so it is
absent from the port the dashboard and management API are served from rather
than merely gated on it. Bind it to loopback or a private interface. The
gateway sends `ROLTER_INTERNAL_TOKEN` on snapshot polls, falling back to
`ROLTER_ADMIN_TOKEN`.

Both are optional. Unset, the historical behavior holds — `/internal/*` shares
the public listener and accepts the admin token — and the control plane logs a
warning at startup saying so. The tenant-facing CRUD surface never returns a
provider key in any configuration.

Two things this deliberately does **not** do. There is no mTLS between the
planes: a shared secret over a private interface is comparable strength when
the network path is already trusted, and mTLS adds certificate lifecycle to an
air-gapped deployment. And the snapshot still carries plaintext rather than
sealed ciphertext: envelope-passing would require the gateway to hold the KEK,
moving the master secret onto every data-plane node, which is a worse trade for
most deployments. Revisit both if the planes ever cross an untrusted network.

## Wire transparency

- Outbound requests to upstream providers carry **no rolter-identifying marks**: no `User-Agent`, no added `X-*`/`Via` headers, no metadata injected into the JSON body, no marks in SSE framing. The only headers sent are functionally required ones — `content-type`, the provider's auth header, and `anthropic-version` for Anthropic.
- Responses back to clients likewise gain no rolter-added headers.
- This is a tested guarantee: golden wire tests in `rolter-proxy` capture the raw outbound request head and fail on any unexpected header (see `openai_wire_carries_no_rolter_signature`).

## External PII sanitization (#848)

The gateway can hand request content — and optionally response content — to a self-hosted de-identification service before it reaches a provider. Configured under `[pii_sanitizer]`; see `docs/user-docs/security/pii-sanitizer.mdx` for the operator-facing contract.

The design property worth stating precisely: **the placeholder→plaintext mapping never enters the gateway process.** The sanitizer substitutes deterministic placeholders and returns an opaque restoration token; rolter holds the token and nothing else. There is therefore no log line, metric label, span attribute or cached body from which the original values can be recovered, and no in-memory table for a crash dump to expose.

Consequences that fall out of that choice:

- **`RestorationTicket` is not printable.** Its `Display` renders `<restoration token redacted>`, and the token is reachable only through `token_for(&scope)`, which returns `None` unless the caller's org/team/project/route match the scope the ticket was minted under. A token from one project cannot restore content in another.
- **Restoration is opt-out, not opt-in-by-default.** `RestorationPolicy::Never` is the default; `CallerAuthorized` honours `x-rolter-pii-restore` only because the _policy_ allows it, never because the caller asked.
- **The response leg never requests reversibility.** Restoring provider-generated content would return the very data that leg exists to remove.
- **A streamed response with an active response leg is refused** (`pii_streaming_unsupported`, HTTP 400), mirroring `guardrails.streaming_post_call`. Correct streaming restoration would need either a network round trip per chunk — destroying TTFT — or the mapping held in-process, destroying the property above. `streaming = "passthrough"` waives the response leg instead.
- **A malformed sanitizer reply is a failure**, unlike the guardrail webhook's decision parsing which defaults to allow. Defaulting would forward content the gateway believes is sanitized and is not.
- **Restore runs after the cache store**, so what is cached is what the upstream said. A later hit on the same entry is re-evaluated against its own request's policy and ticket rather than replaying someone else's restored plaintext.
- **`fail_open` is the default and is the risk to monitor.** It forwards unsanitized content when the service is down. `rolter_pii_sanitizer_errors_total` rising under `fail_open` means personal data is reaching providers; deployments where the sanitizer is a compliance control should set `fail_closed`.

A response-leg failure never fails the request even under `fail_closed`: the upstream call already happened and was already billed, so refusing to deliver would spend the caller's money and return nothing. The counter records it.

## Streamed responses and post-response controls (#1776)

Output guardrails, `post_response` plugins and the PII sanitizer's response leg all judge a whole response body, and a stream reaches the caller before any whole body exists. A setting that the client flips with `"stream": true` must not switch off a control the operator configured, so each of the three refuses the stream up front with a `400` and `param: "stream"`, before the cache lookup or the upstream call. Each has an explicit way to waive the refusal.

| Control                | Refused with                      | Counter                                    | Waived by                                        |
| ---------------------- | --------------------------------- | ------------------------------------------ | ------------------------------------------------ |
| output guardrails      | `guardrail_streaming_unsupported` | `rolter_guardrail_stream_rejections_total` | `guardrails.streaming_post_call = "passthrough"` |
| `post_response` plugin | `plugin_streaming_unsupported`    | `rolter_plugin_stream_rejections_total`    | the plugin's `failure_mode = "fail_open"`        |
| PII response leg       | `pii_streaming_unsupported`       | `rolter_pii_stream_rejections_total`       | `pii_sanitizer.streaming = "passthrough"`        |

A plugin's waiver is its own failure mode rather than a new setting. `fail_closed` already says "never deliver what I have not approved", and a stream is exactly a response the plugin cannot approve. `fail_open` already says "deliver when I cannot answer", which is all a stream permits. The check (`handlers.rs`, beside `post_response_plugin_list`) refuses when any applicable plugin is fail closed and names it in the message. Before #1776, every `post_response` plugin was silently skipped for a stream. Tests: the `*_post_response_plugin_*stream*` cases in `crates/rolter-gateway/tests/integration.rs`, including a cached stream refused after a hot reload adds the plugin.

The dashboard states the consequence where the choice is made (#2178). `ui/src/pages/Plugins.tsx` shows a note in the plugin dialog once the stage is `post_response`, worded by the selected `failure_mode` and wired as the description of both pickers, and a `post_response` card with `fail_closed` carries a one-line warning. Unlike the guardrail rule screen, which has to read `streaming_post_call` from the effective config, both follow from the plugin's own fields. The stories `WarnsThatAFailClosedPostResponsePluginRefusesStreams` and `DialogNoteTracksStageAndFailurePolicy` pin it.

## Failed-login throttling (#1079)

`POST /api/v1/auth/login` is unauthenticated and runs one argon2id verification
per request. That cost is deliberate against guessing and accidental against
denial of service: an attacker who does not care about the password can saturate
the control plane by posting junk credentials. `crates/rolter-control/src/login_throttle.rs`
counts rejected attempts and refuses further ones **before** the password is
verified, which is what closes the CPU half rather than only the guessing half.

Four properties are load-bearing:

- **The counters key on the submitted address, never on a row that was found.**
  An unregistered address is counted, delayed and locked exactly like a real
  one, so the throttle does not undo the constant-cost verification in
  `LocalIdentityProvider::resolve` by becoming an enumeration oracle.
- **A lock is a clock, not a state.** It is a TTL nobody clears by hand. A lock
  an attacker could _set_ would be a denial-of-service primitive against the
  operator — anyone who knows an email address could park the account. The worst
  case here is a bounded outage capped by `--login-max-lock-secs`.
- **Two independent subjects.** The account budget stops guessing one password;
  the client-address budget stops spraying one guess across many accounts, which
  a per-account counter never sees. `X-Forwarded-For` names the client only when
  `--login-trust-forwarded-for` is set, because an unverified forwarded header
  hands out unlimited budgets _and_ lets one client spend another's.
- **A backend outage degrades to absent, not to locked.** A redis error counts
  as zero failures. The alternative — treating an unreachable counter as a
  reason to refuse — would turn one redis blip into a fleet-wide lockout.

The same guard covers the invitation preview/accept endpoints, which are the
same primitive with a different token.

Rejected attempts, engaged locks and sign-ins that follow a lock are written to
`audit_log`, and `rolter_control_login_attempts` counts them by outcome
(`success`, `invalid`, `throttled`, `locked`, `error`) with no account or address
label. The audit write is spawned rather than awaited: resolving the org an
entry belongs to costs a query that only exists when the account does, and
awaiting it would make a registered address measurably slower to reject.

Counters live in redis when one is configured, so every replica shares a budget,
and in-process otherwise. That fallback is documented rather than hidden: with N
replicas and no redis, an attacker gets N times the allowance, and the control
plane says so at startup.

## Who reads account events (#1854)

Sign-ins, failed sign-ins, lockouts, second-factor changes, break-glass resets
and account edits belong to a person, and a person can belong to several orgs,
so these rows are written to `audit_log` with no org. Until #1854 the only read
path, `GET /api/v1/orgs/{org_id}/audit-log`, filtered on the org column, so none
of them reached an API or a screen.

The org read now joins on membership: a row with no org is returned to an org
when its actor, or its target user, holds a role in the org, one of its teams
or one of its projects. That puts an org member's failed sign-ins, a second
factor being removed and a break-glass reset in front of the people who
administer that org, in **Governance → Audit Logs**, where the `auth.*` actions
are filterable. It never shows one org another org's people. `user.delete` is
the exception to "written with no org": it is written once per org the account
belonged to, because the account's memberships are deleted with it and the
join would have nothing left to match.

The alternative was to write each account event once per org the person
belongs to. It was not taken because it fans one sign-in out into a row per
org, and it would leave every row already written unreadable, while the join
makes a deployment's existing history readable as soon as the control plane is
upgraded. The cost is that visibility follows current membership: an org sees
the account events of the people it has now, including events from before they
joined, and stops seeing someone's once they hold no role in it. Deactivation
keeps memberships, so a deactivated leaver stays visible; `user.delete`, which
removes them, is written per org for that reason.

Rows no org can claim — the account events of someone with no membership
anywhere, above all a superadmin's own sign-ins, and attempts against an address
nobody registered, which are recorded with no actor — are read through
`GET /api/v1/audit-log` (#1858). It returns every row in `audit_log`, org-less
ones included, and is guarded by the superadmin-only `deployment_audit_log:read`
capability. It is the same keyset-paged read as the per-org one: the handlers
share the query-string parsing (`audit_log_filter`), the `(at, id)` cursor and
the response builder, and differ only in the store call (`list_page_all` /
`count_all` have no org predicate). A security-auditor role (#1834) does not
exist yet, so the endpoint admits superadmins alone; that role is to be added
at the guard in `list_deployment_audit_log`.

## Three credentials, one word (#943)

Three unrelated secrets pass through rolter, and each is an "API key" to
someone. Keeping them apart is a security property, not a documentation
nicety — a provider key pasted into a client is a credential leak that no
budget, allow-list or audit row constrains.

| Credential   | Shape                    | Held by               | Checked by                         |
| ------------ | ------------------------ | --------------------- | ---------------------------------- |
| virtual key  | `sk-rolter-<48 hex>`     | client applications   | the gateway, on `/v1/*`            |
| provider key | the provider's own shape | rolter                | nothing — it is presented upstream |
| admin token  | operator-chosen          | operators, automation | the control plane                  |

The gateway therefore refuses a _provider-shaped_ key with a message that names
the mistake ("this looks like an Anthropic provider key…") rather than a bare
`invalid api key`. `provider_key_vendor` in
`crates/rolter-gateway/src/handlers.rs` does the shape match, and it never
classifies our own `sk-rolter-` prefix: a revoked or mistyped virtual key is a
different problem and gets the plain message. The hint is derived purely from
the prefix — no lookup, no timing difference between a known and an unknown
key, and the presented secret is never echoed back.

The dashboard follows the same vocabulary: **Virtual Keys** and **My Virtual
Keys** mint the client credential, the provider sheet says _Provider key_, and
the Playground's key field says which one it wants. `docs/user-docs/security/which-key`
is the user-facing version of this table.

The Playground does not ask for that key first. Opening it mints a session key
through `POST /api/v1/me/projects/{id}/playground-key` (see
[RBAC and auth](rbac-and-auth.md#the-playground-key-is-scoped-by-the-server)),
and the dashboard holds the plaintext in a module variable in
`ui/src/lib/gateway.ts` — never in `localStorage`, which is where it used to go
and where a long-lived production key then sat until somebody cleared it
(#944). The screen renders the key's _state_, never the secret: a badge, the
expiry, and a **Renew key** button that asks for a fresh key rather than
extending the one in hand. The paste field stays for testing one specific key
on purpose, and a key pasted there carries no expiry, because the dashboard did
not choose one.

## Threat model (high level)

- **Tenant isolation**: virtual keys are scoped to a project and reach only their own org's routes, providers and provider groups (#1844); model allow-lists prevent access to unconfigured models; cache keys are namespaced to avoid cross-tenant cache poisoning.
- **Offboarding**: deactivating a person, or removing their last role that reaches a project, takes the keys they minted for themselves off every gateway, and deleting the account disables them (#1841); keys an admin minted for an application are the project's and stay. See [RBAC & authentication](rbac-and-auth.md#personal-keys-follow-their-creator-1841).
- **Abuse**: RPM/TPM rate limits and budgets bound spend and load (roadmap enforcement); failed control-plane sign-ins are throttled per account and per client address (see above).
- **AuthZ**: control-plane mutations are RBAC-checked and recorded in `audit_log`.
- **Supply chain**: `cargo deny`/advisory scanning in CI is a roadmap item.

## Operational guidance

- Always set a strong `ROLTER_KEK` (e.g. `openssl rand -hex 32`) and rotate provider keys periodically.
- Run the control plane on a private network; expose only the gateway publicly.
- Back up Postgres; treat the master key as the most sensitive secret. It is
  not in the dump, so a database backup without it restores into a store
  nothing can read — `rolter kek verify` catches that at restore time, and
  [the runbook](../deployment/backup-and-restore.md) covers backup, restore
  and rotation.
