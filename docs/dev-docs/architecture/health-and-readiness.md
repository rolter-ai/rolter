# Health and readiness

Both rolter binaries serve two operational endpoints that answer two different
questions. They are unauthenticated, cheap, and safe to probe on every pod.

| Endpoint | Question | Failure means |
|---|---|---|
| `GET /healthz` | Is the process alive and its runtime not wedged? | Kubernetes **kills and restarts** the pod |
| `GET /readyz` | Can this process actually serve traffic right now? | Kubernetes **removes the pod from the Service** and nothing else |

The words mean the same thing on `rolter-gateway` and `rolter-control`.

## Why the split exists

Collapsing both probes onto one endpoint has no correct answer, only a choice
of which failure to accept:

- If the single endpoint answers as soon as Axum binds, Kubernetes marks the
  pod Ready before it can serve. A rolling update then routes dashboard and
  `/internal/snapshot` traffic — which every gateway in the fleet polls — at a
  control pod whose migrations have not run and whose pool has no connection.
- If the single endpoint instead checks Postgres, a database blip fails the
  *liveness* probe and Kubernetes kills every control pod at once. A
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

Redis and ClickHouse are **deliberately not checked**. The control plane serves
configuration without either — losing Redis costs the config-bump fan-out
notification, losing ClickHouse costs the analytics screens — so their absence
is degraded, not unready. Removing every control pod from the Service because
analytics is down would be a strictly worse outage.

Running without `--database-url` (bootstrap config only), there is nothing to
wait on and the answer is always ready.

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
- **`/readyz` down on a *single* gateway** — usually just a drained instance,
  which is expected during a rolling update. Do not page on this alone.
