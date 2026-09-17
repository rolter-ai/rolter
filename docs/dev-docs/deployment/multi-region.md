# Multi-region deployment

Rolter gateways are stateless. Run one gateway/control pair per region behind a global load balancer, and keep their operational state in shared managed services:

| State | Recommended topology | Failure behaviour |
| --- | --- | --- |
| Postgres | primary with cross-region replica / managed HA | control writes go to the primary; gateways retain their last atomic snapshot during a control outage |
| Redis / Valkey | multi-AZ primary with replica or managed global datastore | response-cache and rate-limit state are shared; a Redis outage degrades to uncached/local operation |
| ClickHouse | replicated cluster or regional ingest plus central query endpoint | observability can lag; request forwarding is never blocked |

Use the same bootstrap configuration and `ROLTER_KEY_PEPPER` in every region. The control plane publishes config-version changes through Redis; gateways also poll `/internal/snapshot`, so a missed pub/sub event converges without a restart.

## Active-active

Deploy at least two regional gateway pools and route client traffic to the closest healthy pool. Point every control plane at the same primary Postgres and every gateway at the same Redis namespace. Configure health checks against `/healthz`; remove an unhealthy region at the global load balancer, not by withdrawing its routes from the shared database.

For provider failover, put independent regional upstream endpoints in one model route. The existing retry, health, cooldown, and circuit-breaker logic then retries the next target. Keep target names stable across regions so logs and health events remain comparable.

## Active-passive

Keep the passive region deployed with the same config and external state endpoints, but give it zero global-LB weight. Promotion is an LB/DNS change after verifying the passive region can reach Postgres, Redis, ClickHouse, and its upstream providers. Do not copy database rows during failover: replication is the source of truth.

## Operational checks

1. Apply ClickHouse migrations in every ingest cluster, including `005_request_payloads.sql` when raw payload capture is enabled.
2. Verify `/internal/snapshot` returns the same config version from each regional control plane.
3. Send a `fake-llm` request through each region and confirm `x-rolter-provider`/`x-rolter-target` headers.
4. Test global-LB withdrawal of one region while a streamed response is in flight; new requests should move regions and existing streams should drain.

Avoid asynchronous multi-primary writes to the same tenancy objects. CRUD changes are serialized through the Postgres primary, while gateways are designed to serve their last good snapshot until the new version arrives.
