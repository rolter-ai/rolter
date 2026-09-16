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

| Setting | Effect on the gateway |
| --- | --- |
| `virtual_key_required` | an unauthenticated request is refused even where the gateway holds no keys |
| `required_headers` | a request missing any name/value pair is refused at ingress, before routing and before auth |
| `auth_bypass_routes` | the named paths answer without a key |

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

## Open mode (no admin token)

With no `ROLTER_ADMIN_TOKEN` set, `Principal` short-circuits to `Superadmin` for
every request: the management API and `/internal/snapshot` have no
authentication step to fail. This is the zero-credential local-dev shape, and
`crates/rolter-control/src/open_mode.rs` is what keeps it from being anything
else. Before either listener is opened, it evaluates "is a token set" against
every address about to be bound:

| admin token | bind | outcome |
|---|---|---|
| set | any | `Closed` — RBAC enforced |
| unset | all listeners loopback | `OpenLoopback` — allowed, warned |
| unset | any non-loopback listener | **refuses to start** |
| unset | non-loopback + `--allow-open-mode` | `OpenAcknowledged` — allowed, warned loudly |

`--internal-addr` counts as a listener here: an exposed credential channel is
no better than an exposed API, so either one alone is enough to refuse.

The decision also rides into the dashboard through `window.__ROLTER_CONFIG__`
(`openMode: true`), which renders a persistent banner. Without it the dashboard
looks identical whether the control plane is gated or wide open, which is the
property that made this dangerous rather than merely permissive (#970).

This is why `ROLTER_CONTROL_HOST` defaults to `127.0.0.1` rather than
`0.0.0.0`: containers and clusters set it explicitly, and by then they have a
reason to have set a token too.

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

The gateway can hand request content — and optionally response content — to a self-hosted de-identification service before it reaches a provider. Configured under `[pii_sanitizer]`; see `user-docs/security/pii-sanitizer.mdx` for the operator-facing contract.

The design property worth stating precisely: **the placeholder→plaintext mapping never enters the gateway process.** The sanitizer substitutes deterministic placeholders and returns an opaque restoration token; rolter holds the token and nothing else. There is therefore no log line, metric label, span attribute or cached body from which the original values can be recovered, and no in-memory table for a crash dump to expose.

Consequences that fall out of that choice:

- **`RestorationTicket` is not printable.** Its `Display` renders `<restoration token redacted>`, and the token is reachable only through `token_for(&scope)`, which returns `None` unless the caller's org/team/project/route match the scope the ticket was minted under. A token from one project cannot restore content in another.
- **Restoration is opt-out, not opt-in-by-default.** `RestorationPolicy::Never` is the default; `CallerAuthorized` honours `x-rolter-pii-restore` only because the *policy* allows it, never because the caller asked.
- **The response leg never requests reversibility.** Restoring provider-generated content would return the very data that leg exists to remove.
- **A streamed response with an active response leg is refused** (`pii_streaming_unsupported`, HTTP 400), mirroring `guardrails.streaming_post_call`. Correct streaming restoration would need either a network round trip per chunk — destroying TTFT — or the mapping held in-process, destroying the property above. `streaming = "passthrough"` waives the response leg instead.
- **A malformed sanitizer reply is a failure**, unlike the guardrail webhook's decision parsing which defaults to allow. Defaulting would forward content the gateway believes is sanitized and is not.
- **Restore runs after the cache store**, so what is cached is what the upstream said. A later hit on the same entry is re-evaluated against its own request's policy and ticket rather than replaying someone else's restored plaintext.
- **`fail_open` is the default and is the risk to monitor.** It forwards unsanitized content when the service is down. `rolter_pii_sanitizer_errors_total` rising under `fail_open` means personal data is reaching providers; deployments where the sanitizer is a compliance control should set `fail_closed`.

A response-leg failure never fails the request even under `fail_closed`: the upstream call already happened and was already billed, so refusing to deliver would spend the caller's money and return nothing. The counter records it.

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
  an attacker could *set* would be a denial-of-service primitive against the
  operator — anyone who knows an email address could park the account. The worst
  case here is a bounded outage capped by `--login-max-lock-secs`.
- **Two independent subjects.** The account budget stops guessing one password;
  the client-address budget stops spraying one guess across many accounts, which
  a per-account counter never sees. `X-Forwarded-For` names the client only when
  `--login-trust-forwarded-for` is set, because an unverified forwarded header
  hands out unlimited budgets *and* lets one client spend another's.
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

## Three credentials, one word (#943)

Three unrelated secrets pass through rolter, and each is an "API key" to
someone. Keeping them apart is a security property, not a documentation
nicety — a provider key pasted into a client is a credential leak that no
budget, allow-list or audit row constrains.

| Credential | Shape | Held by | Checked by |
| --- | --- | --- | --- |
| virtual key | `sk-rolter-<48 hex>` | client applications | the gateway, on `/v1/*` |
| provider key | the provider's own shape | rolter | nothing — it is presented upstream |
| admin token | operator-chosen | operators, automation | the control plane |

The gateway therefore refuses a *provider-shaped* key with a message that names
the mistake ("this looks like an Anthropic provider key…") rather than a bare
`invalid api key`. `provider_key_vendor` in
`crates/rolter-gateway/src/handlers.rs` does the shape match, and it never
classifies our own `sk-rolter-` prefix: a revoked or mistyped virtual key is a
different problem and gets the plain message. The hint is derived purely from
the prefix — no lookup, no timing difference between a known and an unknown
key, and the presented secret is never echoed back.

The dashboard follows the same vocabulary: **Virtual Keys** and **My Virtual
Keys** mint the client credential, the provider sheet says *Provider key*, and
the Playground's key field says which one it wants. `user-docs/security/which-key`
is the user-facing version of this table.

## Threat model (high level)

- **Tenant isolation**: virtual keys are scoped to a project; model allow-lists prevent access to unconfigured models; cache keys are namespaced to avoid cross-tenant cache poisoning.
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
