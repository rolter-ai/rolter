# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [0.0.9](https://github.com/ormeilu/rolter/compare/rolter-core-v0.0.8...rolter-core-v0.0.9) - 2026-07-15

### Features
- *(store)* add immutable URL-safe provider slug for model addressing ([#191](https://github.com/ormeilu/rolter/pull/191)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.8](https://github.com/ormeilu/rolter/compare/rolter-core-v0.0.7...rolter-core-v0.0.8) - 2026-07-13

### Features
- *(proxy)* support custom ca bundles ([#168](https://github.com/ormeilu/rolter/pull/168)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* support responses lifecycle resources ([#166](https://github.com/ormeilu/rolter/pull/166)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* normalize provider role capabilities [ROL-262] ([#164](https://github.com/ormeilu/rolter/pull/164)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add responses api translation ([#162](https://github.com/ormeilu/rolter/pull/162)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* isolate provider queues and backpressure ([#158](https://github.com/ormeilu/rolter/pull/158)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add cloud provider health adapters ([#157](https://github.com/ormeilu/rolter/pull/157)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add TEI embeddings provider ([#154](https://github.com/ormeilu/rolter/pull/154)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add OpenRouter provider ([#153](https://github.com/ormeilu/rolter/pull/153)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add self-hosted ollama provider ([#150](https://github.com/ormeilu/rolter/pull/150)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* proxy realtime websocket sessions ([#156](https://github.com/ormeilu/rolter/pull/156)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* configurable request body-size limit ([#148](https://github.com/ormeilu/rolter/pull/148)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* per-virtual-key response-cache override ([#146](https://github.com/ormeilu/rolter/pull/146)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* cache streaming/SSE responses ([#145](https://github.com/ormeilu/rolter/pull/145)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* exact-match response cache (redis, ttl, per-route opt-in) ([#142](https://github.com/ormeilu/rolter/pull/142)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.6](https://github.com/ormeilu/rolter/compare/rolter-core-v0.0.5...rolter-core-v0.0.6) - 2026-07-12

### Features
- *(balancer)* fastest latency-aware routing strategy ([#130](https://github.com/ormeilu/rolter/pull/130)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* cheapest cost-aware routing strategy ([#128](https://github.com/ormeilu/rolter/pull/128)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* service-info landing on GET / ([#113](https://github.com/ormeilu/rolter/pull/113)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* served openapi document + embedded scalar api reference [ROL-72] ([#111](https://github.com/ormeilu/rolter/pull/111)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* opentelemetry otlp trace export via OTEL_* env [ROL-59] ([#104](https://github.com/ormeilu/rolter/pull/104)) by [@ormeilu](https://github.com/ormeilu)

## [0.0.5](https://github.com/ormeilu/rolter/compare/rolter-core-v0.0.4...rolter-core-v0.0.5) - 2026-07-11

### Added

- *(gateway)* provider status-page secondary health signal [ROL-200] ([#90](https://github.com/ormeilu/rolter/pull/90))
- *(gateway)* opt-in also_track_via_llm_call end-to-end health check ([#89](https://github.com/ormeilu/rolter/pull/89))
- *(gateway)* per-key cooldown + sibling-key failover ([#85](https://github.com/ormeilu/rolter/pull/85))
- *(gateway)* weighted api-key selection per request ([#84](https://github.com/ormeilu/rolter/pull/84))
- *(core)* multiple weighted api keys per provider ([#83](https://github.com/ormeilu/rolter/pull/83))
- *(gateway)* probe guardrails — concurrency cap, jitter, 429 backoff, flip thresholds ([#82](https://github.com/ormeilu/rolter/pull/82))

## [0.0.4](https://github.com/ormeilu/rolter/compare/rolter-core-v0.0.3...rolter-core-v0.0.4) - 2026-07-10

### Added

- *(gateway)* wire variant routing into the request/failover loop ([#67](https://github.com/ormeilu/rolter/pull/67))
- *(gateway)* configurable metrics path to avoid scrape collisions ([#66](https://github.com/ormeilu/rolter/pull/66))
- *(core)* weighted variant abstraction with ordered fallback ([#65](https://github.com/ormeilu/rolter/pull/65))
- *(gateway)* per-model default inference params with admin override policy ([#61](https://github.com/ormeilu/rolter/pull/61))
- *(gateway)* scrape upstream engine /metrics into a lock-free load signal ([#60](https://github.com/ormeilu/rolter/pull/60))
- *(balancer)* wire the scorer pipeline in as a selectable `pipeline` strategy ([#59](https://github.com/ormeilu/rolter/pull/59))
- *(gateway)* per-target circuit breaker (closed/open/half-open) [ROL-47] ([#57](https://github.com/ormeilu/rolter/pull/57))
- *(gateway)* active upstream health checks skipping unhealthy targets [ROL-49] ([#51](https://github.com/ormeilu/rolter/pull/51))
- *(balancer)* weighted selection honoring Target.weight [ROL-51] ([#50](https://github.com/ormeilu/rolter/pull/50))
- *(gateway)* upstream timeouts + graceful shutdown/drain [ROL-52] ([#48](https://github.com/ormeilu/rolter/pull/48))
- *(gateway)* per-target cooldowns on transient failures [ROL-48] ([#47](https://github.com/ormeilu/rolter/pull/47))
- *(gateway)* configurable retries with backoff + jitter [ROL-46] ([#46](https://github.com/ormeilu/rolter/pull/46))
- *(gateway)* rpm/tpm rate limits with redis sliding window ([#42](https://github.com/ormeilu/rolter/pull/42))

### Other

- expand quality gate into a hardened multi-check pipeline [ROL-124] ([#54](https://github.com/ormeilu/rolter/pull/54))
- *(core)* expand config validation and enumerate startup problems [ROL-89] ([#53](https://github.com/ormeilu/rolter/pull/53))

## [0.0.3](https://github.com/ormeilu/rolter/compare/rolter-core-v0.0.2...rolter-core-v0.0.3) - 2026-07-09

### Added

- *(gateway)* budget enforcement with redis spend counters ([#37](https://github.com/ormeilu/rolter/pull/37))

## [0.0.2](https://github.com/ormeilu/rolter/compare/v0.0.1...v0.0.2) - 2026-07-02

### Added

- *(store)* postgres-backed ConfigStore with sqlx migration runner [ROL-20 ROL-21] ([#9](https://github.com/ormeilu/rolter/pull/9))
