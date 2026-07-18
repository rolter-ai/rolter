# Per-provider egress proxy pools

## Metadata

| Field | Value |
| --- | --- |
| Product | rolter |
| Date | 18 Jul 2026 |
| Status | ACCEPTED |
| Issues | [#305](https://github.com/rolter-ai/rolter/issues/305) |

## Context

A provider may need several outbound proxies for regional reachability, IP-based
rate limits, or resilience. The existing singular `egress_proxy` setting cannot
spread traffic or recover when its proxy becomes unavailable. Proxy credentials
must not enter database rows, config snapshots, API responses, or metrics labels.

## Options considered

1. Keep one proxy and rely on an external proxy load balancer.
2. Select a random proxy independently for every attempt.
3. Maintain a per-provider round-robin pool with local failure quarantine.

## Decision

Adopt option 3. `egress_proxies` is the canonical ordered pool, while the legacy
`egress_proxy` remains a backward-compatible one-element pool. Each member owns a
cached `reqwest` client and may use HTTP, HTTPS, SOCKS5, or SOCKS5H. Requests start
at the next round-robin member and retry connect or tunnel failures through the
remaining eligible members. Three consecutive failures quarantine a member for
30 seconds; a successful request clears its failure streak.

Authenticated URLs are accepted only as whole-value `${ENV_VAR}` references.
Metrics identify members with a redacted stable label and never expose userinfo.

## Consequences

- Providers can spread traffic and survive an individual proxy failure without a
  separate egress load balancer.
- Existing singular configuration continues to work unchanged.
- Retry is deliberately limited to connection/tunnel failures; retrying arbitrary
  HTTP responses could duplicate a request whose upstream already processed it.
- Quarantine state and round-robin position are process-local, so gateway replicas
  may temporarily make different choices.
- Operators must inject authenticated proxy URLs into every gateway replica.
