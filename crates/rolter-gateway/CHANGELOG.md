# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.5](https://github.com/ormeilu/rolter/compare/v0.0.4...v0.0.5) - 2026-07-11

### Added

- *(gateway)* propagate caller trace context to upstream ([#96](https://github.com/ormeilu/rolter/pull/96))
- *(gateway)* end-to-end request id + inbound trace continuation [ROL-60] ([#95](https://github.com/ormeilu/rolter/pull/95))
- *(gateway)* provider status-page secondary health signal [ROL-200] ([#90](https://github.com/ormeilu/rolter/pull/90))
- *(gateway)* opt-in also_track_via_llm_call end-to-end health check ([#89](https://github.com/ormeilu/rolter/pull/89))
- *(gateway)* provider_health_events clickhouse table + async writer ([#86](https://github.com/ormeilu/rolter/pull/86))
- *(gateway)* per-key cooldown + sibling-key failover ([#85](https://github.com/ormeilu/rolter/pull/85))
- *(gateway)* weighted api-key selection per request ([#84](https://github.com/ormeilu/rolter/pull/84))
- *(core)* multiple weighted api keys per provider ([#83](https://github.com/ormeilu/rolter/pull/83))
- *(gateway)* probe guardrails — concurrency cap, jitter, 429 backoff, flip thresholds ([#82](https://github.com/ormeilu/rolter/pull/82))
- *(gateway)* strategy-aware target selection within variants ([#80](https://github.com/ormeilu/rolter/pull/80))
- *(gateway)* per-variant request counter in /metrics [ROL-195] ([#79](https://github.com/ormeilu/rolter/pull/79))
- *(gateway)* kind-aware free liveness probes for active health checks [ROL-123] ([#78](https://github.com/ormeilu/rolter/pull/78))
- *(gateway)* passive per-target SLA counters in /metrics [ROL-194] ([#77](https://github.com/ormeilu/rolter/pull/77))

### Other

- *(gateway)* built-in fake-llm and config hot-reload integration coverage ([#91](https://github.com/ormeilu/rolter/pull/91))

## [0.0.4](https://github.com/ormeilu/rolter/compare/v0.0.3...v0.0.4) - 2026-07-10

### Added

- *(gateway)* wire variant routing into the request/failover loop ([#67](https://github.com/ormeilu/rolter/pull/67))
- *(gateway)* configurable metrics path to avoid scrape collisions ([#66](https://github.com/ormeilu/rolter/pull/66))
- *(core)* weighted variant abstraction with ordered fallback ([#65](https://github.com/ormeilu/rolter/pull/65))
- *(gateway)* per-model default inference params with admin override policy ([#61](https://github.com/ormeilu/rolter/pull/61))
- *(gateway)* scrape upstream engine /metrics into a lock-free load signal ([#60](https://github.com/ormeilu/rolter/pull/60))
- *(gateway)* per-target circuit breaker (closed/open/half-open) [ROL-47] ([#57](https://github.com/ormeilu/rolter/pull/57))
- *(gateway)* active upstream health checks skipping unhealthy targets [ROL-49] ([#51](https://github.com/ormeilu/rolter/pull/51))
- *(balancer)* weighted selection honoring Target.weight [ROL-51] ([#50](https://github.com/ormeilu/rolter/pull/50))
- *(gateway)* in-flight load counters feeding the balancer [ROL-50] ([#49](https://github.com/ormeilu/rolter/pull/49))
- *(gateway)* upstream timeouts + graceful shutdown/drain [ROL-52] ([#48](https://github.com/ormeilu/rolter/pull/48))
- *(gateway)* per-target cooldowns on transient failures [ROL-48] ([#47](https://github.com/ormeilu/rolter/pull/47))
- *(gateway)* configurable retries with backoff + jitter [ROL-46] ([#46](https://github.com/ormeilu/rolter/pull/46))
- *(gateway)* rpm/tpm rate limits with redis sliding window ([#42](https://github.com/ormeilu/rolter/pull/42))

### Other

- *(gateway)* end-to-end integration suite with mock upstreams + SSE ([#64](https://github.com/ormeilu/rolter/pull/64))
- *(gateway)* structured OpenAI-style error responses everywhere [ROL-88] ([#56](https://github.com/ormeilu/rolter/pull/56))
- *(core)* expand config validation and enumerate startup problems [ROL-89] ([#53](https://github.com/ormeilu/rolter/pull/53))

## [0.0.3](https://github.com/ormeilu/rolter/compare/v0.0.2...v0.0.3) - 2026-07-09

### Added

- *(gateway)* budget enforcement with redis spend counters ([#37](https://github.com/ormeilu/rolter/pull/37))

## [0.0.2](https://github.com/ormeilu/rolter/compare/v0.0.1...v0.0.2) - 2026-07-02

### Added

- *(gateway)* reload-free config watcher polling the control plane ([#18](https://github.com/ormeilu/rolter/pull/18))
- *(gateway)* ship built-in fake-llm default model ([#14](https://github.com/ormeilu/rolter/pull/14))
- *(core)* scaffold rolter workspace and runnable gateway mvp

### Fixed

- *(gateway)* enforce virtual-key auth on GET /v1/models ([#8](https://github.com/ormeilu/rolter/pull/8))

### Other

- release v0.0.1 ([#3](https://github.com/ormeilu/rolter/pull/3))

## [0.0.1](https://github.com/ormeilu/rolter/releases/tag/v0.0.1) - 2026-06-30

### Added

- *(core)* scaffold rolter workspace and runnable gateway mvp
