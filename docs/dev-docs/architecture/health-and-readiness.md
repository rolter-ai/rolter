# Health and readiness

Both rolter binaries serve two operational endpoints that answer two different
questions. They are unauthenticated, cheap, and safe to probe on every pod.

| Endpoint       | Question                                           | Failure means                                                    |
| -------------- | -------------------------------------------------- | ---------------------------------------------------------------- |
| `GET /healthz` | Is the process alive and its runtime not wedged?   | Kubernetes **kills and restarts** the pod                        |
| `GET /readyz`  | Can this process actually serve traffic right now? | Kubernetes **removes the pod from the Service** and nothing else |

The words mean the same thing on `rolter-gateway` and `rolter-control`.

## Why the split exists

Collapsing both probes onto one endpoint has no correct answer, only a choice
of which failure to accept:

- If the single endpoint answers as soon as Axum binds, Kubernetes marks the
  pod Ready before it can serve. A rolling update then routes dashboard and
  `/internal/snapshot` traffic — which every gateway in the fleet polls — at a
  control pod whose migrations have not run and whose pool has no connection.
- If the single endpoint instead checks Postgres, a database blip fails the
  _liveness_ probe and Kubernetes kills every control pod at once. A
  recoverable dependency outage becomes a restart storm, and the restarts make
  the outage worse by reconnecting a stampede at the recovering database.

Splitting the endpoints lets a dependency failure drain traffic without
touching the process.

## `/healthz` — liveness

Returns `200 OK` with the body `ok` whenever the HTTP server is accepting
connections. It checks **no dependency**: not Postgres, not Redis, not
ClickHouse, not an upstream provider. That is the point — nothing outside the
process may cause a restart.

## `/readyz` — readiness

### Gateway

Returns `200 OK` normally, and `503 Service Unavailable` with the body
`draining` once the instance has been drained from the dashboard's cluster
screen — the state arrives on the `/internal/snapshot` response the gateway
already polls. A drained gateway therefore leaves the Service and finishes
its in-flight requests, instead of being killed with them in progress.

It also answers `503` with the body `snapshot_format_unsupported` while it has
**never** applied a snapshot and the control plane's last answer was a format
it cannot read (ADR-0038, `AppState::unready_for_snapshot_format`). Such a pod
serves its bootstrap file and refuses every key, so removing it from the
Service costs nothing, and it is what holds a Kubernetes rollout whose new
gateways start before a new control plane: the new pod never becomes ready, so
the old ones keep serving. A gateway that _has_ applied a snapshot stays ready
on a refusal and keeps its last config, since flipping it would turn one
out-of-order control-plane upgrade into an outage of every gateway at once;
`rolter_snapshot_format_refused` is the signal for that case. `draining` wins
when both hold.

More generally, a gateway started with `--snapshot-url` answers `503` with the
body `awaiting_snapshot` until it applies its first snapshot
(`AppState::awaiting_first_snapshot`, #3348). Until then it serves its
bootstrap file, and a managed gateway treats an empty key set as closed, so it
refuses every `/v1` request: a Service would send traffic to a pod that turns
all of it away for as long as the control plane is slow or unreachable at
start. Two cases are ready all the same, because they can serve: a bootstrap
file that carries virtual keys, and one that sets `server.require_auth = false`
on purpose. Once a snapshot is applied (`rolter_snapshot_format` is no longer
`0`), the gateway stays ready on every later failed poll and keeps its last
config, as before. `snapshot_format_unsupported` is the more specific answer of
the two and wins when both hold, since it names why no snapshot arrived; a
gateway without `--snapshot-url` never answers either.

### Control plane

Returns a JSON body describing each check:

```json
{
  "status": "ready",
  "checks": {"database": "ok", "migrations": "ok", "kek": "ok"}
}
```

`200` when `status` is `ready`, `503` otherwise. The checks are:

- **database** — the Postgres pool hands out a connection within 2 seconds. The
  timeout is deliberately shorter than a typical probe `timeoutSeconds` so an
  exhausted pool answers a clean `503` rather than hanging until kubelet
  reports a timeout, which looks like a dead process.
- **migrations** — every migration embedded in this binary is applied to the
  database. A pod running ahead of its schema would serve wrong answers on the
  CRUD API and `/internal/snapshot`, so it is not ready.
- **kek** — `ROLTER_KEK` parses when it is set. A KEK set to an empty value
  seals nothing and decrypts nothing, so the pod cannot serve credentials.
  Reported as `not configured` (and not a failure) when the variable is unset.

The probe answers anyone, so a failing check reports the fixed word
`unavailable` rather than the driver's error, which can name the database host
and port (#1840); the detail is in the control plane's log.

Redis and ClickHouse are **deliberately not checked**. The control plane serves
configuration without either — losing Redis costs the config-bump fan-out
notification, losing ClickHouse costs the analytics screens — so their absence
is degraded, not unready. Removing every control pod from the Service because
analytics is down would be a strictly worse outage.

Running without `--database-url` (bootstrap config only), there is nothing to
wait on and the answer is always ready.

## The dashboard's gateway pill

The status pill in every screen header (`ui/src/components/ScreenHeader.tsx`) and the Gateways tile
on the Dashboard home (`ui/src/components/FleetHealthTile.tsx`) read one query, `useGatewayHealth`
in `ui/src/lib/gateway-health.ts`, so the two cannot disagree (#1973, #3263). Every 30 seconds it
asks `GET /api/v1/cluster/summary` (#2013), the roll-up every signed-in caller may read: the live,
stale and config-lagging gateways (`docs/user-docs/api/cluster-summary.mdx` has the contract). The pill says how many are live and, when some
are not, how many are behind or stale; `fleetHealth` maps the counts onto the pill's states
(healthy, degraded while any live gateway lags or any is stale, down when none is live).

`fetchGatewayCheck` falls back to the gateway's `/readyz`, described below, whenever the summary has
nothing to count: `inventory: false` (a control plane with no database), an inventory with no live
or stale gateway (a gateway that sends no node id never enters it), or a failed request (an older
control plane answers `404`). That keeps the pill as informative as it was before the summary
existed, and never reports an empty fleet for a deployment that was simply not counted.

## The `/readyz` probe behind the pill

The probe polls the gateway's `/readyz` through the control plane's `/gw/*` proxy.

`readinessFrom` in `ui/src/lib/gateway.ts` reads the answer strictly:

| Answer                                                    | Pill                                            |
| --------------------------------------------------------- | ----------------------------------------------- |
| `200` with the body `ok`                                  | healthy (green dot, breathing)                  |
| `503` with the body `draining`                            | degraded: "gateway draining", amber             |
| `503` with the body `snapshot_format_unsupported`         | down: "gateway cannot read config", red (#3350) |
| `503` with the body `awaiting_snapshot`                   | down: "gateway has no config", red (#3350)      |
| `502` with the proxy's own `{"error":{"message":…}}` body | down: "gateway unreachable", red                |
| anything else, a network error, or no answer within 10 s  | unknown, grey                                   |
| no answer yet                                             | checking, grey                                  |

A gateway with no config refuses every key, so both `503` bodies that say so
read as down; `gatewayHealthFrom` keeps the probe's answer as `refusal`, and the
pill and the Dashboard's Gateways tile word it on its own rather than as
"unreachable", which would send the operator to the network. The
`snapshot_format_unsupported` tooltip points at the upgrade order
(`/deployment/upgrading#upgrade-order`).

A `200` that is not `ok` is an SPA fallback in front of the control plane, and
a `502` that is not the proxy's JSON comes from an ingress that could not reach
the _control plane_. Both read as unknown, so the pill never sends an operator
to the wrong process.

One failed check does not blank a known answer. `gatewayHealthFrom` in
`ui/src/lib/gateway-health.ts` keeps it for up to one and a half poll
intervals, without the pulse and with the answer's time in the tooltip, then
drops to unknown. The poll pauses while the tab is hidden, and the header's
refresh button re-asks along with everything else without spinning for the
background poll.

What the pill cannot see:

- other gateways behind the same `ROLTER_GATEWAY_URL`, and whether a gateway
  runs the current config version. The probe cannot; the cluster summary above
  can, and the pill uses it first.
- an unreachable gateway _host_ is not a blind spot: the proxy's HTTP client
  bounds the connect at 3 seconds (#2014), so a host that drops packets answers
  the same `502` as a refused port and the pill reads down. Only connecting is
  bounded; streaming responses stay open.

## The circuit breaker's half-open probe (#3248)

A target's breaker (`rolter-gateway/src/breaker.rs`) is closed, open or half-open. Once an open window (`[breaker].open_secs`) elapses, the next `Breaker::allows` for the target moves it to half-open and admits that one request as the **probe**. Until the probe reports back, every other `allows` for the same `(model, target)` is refused, and `Breaker::open_mask` lists the target as refusing, so the balancer weighs the siblings instead. The probe's success (`on_success`, any non-retryable status) closes the breaker; its failure (`on_failure`, a 429, a 5xx or a connection error) re-opens it for a full window. Before #3248 a half-open target admitted every request that arrived before the probe's result, so a still-failing target took a burst of real traffic each time its window elapsed.

The probe holds its slot on a lease, stored in the phase itself (`Phase::HalfOpen(lease_expiry)`, behind the registry's existing lock, no allocation). The lease is the longest of `open_secs`, `PROBE_LEASE_FLOOR` (30 s) and the upstream request timeout (`Breaker::set_probe_timeout`, set from `[timeouts].connect_secs + request_secs` with the longest route `limits.timeout_secs`, on start and every reload): a probe reports back when its response headers arrive, which for a non-streaming completion is when it finishes, so a shorter lease would admit a second probe while the first is still waiting. A probe that ends on a path that says nothing about the target gives its slot back at once with `Breaker::release`, which returns the target to open with its window already elapsed and counts no failure: the target's provider is missing from the snapshot, the provider queue refused admission, a multi-key provider's key answered `429`/`401` (the same target is re-picked with another key), or a refusal handed the request to a fallback that takes it over (#1584). These exits are released in the classic, multipart and variant forward loops. What is left to the lease is a probe whose caller went away mid-request.

A realtime session feeds the breaker too. It picks each candidate before deciding to dial it (one at a time since #3322, so the pick can follow the last connect's failure class), so it reads the breaker without taking a slot (`TargetPool::pick_read_only`, `Breaker::refuses`), asks `allows` only of the target it dials, and closes the breaker on a good handshake (`on_success`, counted in `rolter_breaker_closed_total`) as a good HTTP response does.

The fail-open rule is unchanged: when every target of a route is parked, unhealthy or refusing behind its breaker, `TargetPool::pick` still forwards to the first untried target (`first_untried_allowed`, and the variant loop's `find(open)` fallback, neither of which asks `allows`), so a single-target route, or one whose every target is skipped, keeps sending its traffic to a probing target rather than answering a gateway error. The maintainer decided on #3389 that fail-open keeps winning: a single-target route therefore gets no stampede protection from the half-open probe limit, since exempting a probing target would make such a route answer `503` while its upstream recovers. `handlers::tests::pick_untried_fails_open_past_a_probe_in_flight` pins it.

## The fleet's circuit breakers (#2254)

Breaker state is per process: every gateway replica holds its own registry (`rolter-gateway/src/breaker.rs`), so one replica can shed a target another still uses. The control plane learns it the way it learns the adaptive-routing scores — each gateway **pushes** a sample every 10s to `POST /internal/breaker-state` (`rolter-gateway/src/breaker_report.rs`), over the internal token and the `x-rolter-node-id` identity of the snapshot poll, so a node is one row here and in the cluster inventory. The sample is taken by a background task under the registry lock once per tick; the request path is untouched.

A report replaces the node's whole set of rows in `breaker_state`, so a breaker that closes leaves the table, and **an absent target means closed**. The control plane serves the table at `GET /api/v1/breaker-state` (`rolter-control/src/breaker_state.rs`): rows older than 40s are not served, and `gateways_live` (from the cluster inventory) is what lets a reader tell "all closed" from "nobody is reporting". Two orgs may each have a route of one name (#1857), so a gateway reports each entry as the route's name and owning org rather than its runtime namespace, and a row is keyed by `(node, org_id, model, target)` (#3458). Entries are filtered to the routes, as `(org, model)`, that the caller sees in the catalog, and per-node ids go only to a caller who sees everything.

## Probe wiring

`charts/rolter` wires this for you:

```yaml
readinessProbe:
  httpGet: {path: /readyz, port: http}
livenessProbe:
  httpGet: {path: /healthz, port: http}
startupProbe:               # control plane only
  httpGet: {path: /healthz, port: http}
```

The `startupProbe` on the control plane holds liveness off while first-boot
migrations run against a cold database, so a slow migration is not mistaken for
a wedged process. Tune the periods under `control.probes` in `values.yaml`.

## What to alert on

- **`/readyz` down for longer than a failover** — the control plane cannot
  reach its database, or is running against a schema it has not migrated. Page.
- **`/healthz` flapping** — the process itself is crashing or wedging. Page.
- **`/readyz` down on a _single_ gateway** — usually just a drained instance,
  which is expected during a rolling update. Do not page on this alone.
- **`rolter_snapshot_format_refused` at `1`** — a gateway leads its control
  plane, or its format was retired. It keeps its last config but stops
  receiving changes, revocations included. Ticket, and page if it lasts past a
  rollout.
