# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [0.0.11](https://github.com/rolter-ai/rolter/compare/v0.0.10...v0.0.11) - 2026-08-07

### Bug Fixes
- *(gateway)* surface queue_full on a shed request instead of a 502 [#639] ([#771](https://github.com/rolter-ai/rolter/pull/771)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* drop the inert guardrail default_on flag ([#759](https://github.com/rolter-ai/rolter/pull/759)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* re-check the egress policy at connect time ([#662](https://github.com/rolter-ai/rolter/pull/662)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* make model price currency real and extensible ([#661](https://github.com/rolter-ai/rolter/pull/661)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* split the snapshot channel from the operator API ([#660](https://github.com/rolter-ai/rolter/pull/660)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* fail closed when the virtual-key set is empty ([#653](https://github.com/rolter-ai/rolter/pull/653)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(gateway)* conform to OTel GenAI semantic conventions [#808] ([#829](https://github.com/rolter-ai/rolter/pull/829)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* export logs, process and resource telemetry over OTLP [#809] ([#828](https://github.com/rolter-ai/rolter/pull/828)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* stamp tenant identity on exported spans ([#839](https://github.com/rolter-ai/rolter/pull/839)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway,core)* export labelled counters and histograms over OTLP [#805] ([#822](https://github.com/rolter-ai/rolter/pull/822)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway,infra)* fix trace parenting, span the pipeline, add an observability overlay [#805] ([#806](https://github.com/rolter-ai/rolter/pull/806)) by [@ormeilu](https://github.com/ormeilu)
- *(ui,control)* build client and model settings screens [#564] ([#804](https://github.com/rolter-ai/rolter/pull/804)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* enforce access-profile model and route policy [#791] ([#803](https://github.com/rolter-ai/rolter/pull/803)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* enforce MCP OAuth authorization ([#773](https://github.com/rolter-ai/rolter/pull/773)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* expose adaptive routing telemetry to the control plane ([#765](https://github.com/rolter-ai/rolter/pull/765)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* export adaptive routing decision telemetry ([#705](https://github.com/rolter-ai/rolter/pull/705)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add adaptive routing strategy ([#701](https://github.com/rolter-ai/rolter/pull/701)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* drain nodes out of service safely ([#700](https://github.com/rolter-ai/rolter/pull/700)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add cluster node inventory ([#699](https://github.com/rolter-ai/rolter/pull/699)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* manage cross-dialect compatibility policy ([#696](https://github.com/rolter-ai/rolter/pull/696)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* cap spend by business unit and customer ([#695](https://github.com/rolter-ai/rolter/pull/695)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* log business unit and customer attribution ([#689](https://github.com/rolter-ai/rolter/pull/689)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* attribute virtual-key spend to business units ([#688](https://github.com/rolter-ai/rolter/pull/688)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* register Gemini Interactions provider kind by [@ormeilu](https://github.com/ormeilu)
- *(control)* persist global runtime policy settings by [@ormeilu](https://github.com/ormeilu)
- *(store)* merge published prompt templates into snapshots by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* enforce post_call guardrails on non-streaming responses ([#667](https://github.com/rolter-ai/rolter/pull/667)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* per-route guardrail enable/disable overrides ([#664](https://github.com/rolter-ai/rolter/pull/664)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* expand provider adapter kind coverage [ROL-132] ([#645](https://github.com/rolter-ai/rolter/pull/645)) by [@ormeilu](https://github.com/ormeilu)

### Performance
- *(gateway)* eliminate intermediate allocations in inject_anthropic [ROL-101] ([#787](https://github.com/rolter-ai/rolter/pull/787)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* Pre-allocate vectors in realtime and telemetry modules [ROL-PERF] ([#746](https://github.com/rolter-ai/rolter/pull/746)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* batch redis rate limit commands [ROL-PERF] ([#727](https://github.com/rolter-ai/rolter/pull/727)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* batch budget redis reads with mget ([#728](https://github.com/rolter-ai/rolter/pull/728)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* [performance improvement] [ROL-PERF] ([#730](https://github.com/rolter-ai/rolter/pull/730)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* [performance improvement] [ROL-PERF] ([#726](https://github.com/rolter-ai/rolter/pull/726)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* reduce semantic cache key allocations by [@google-labs-jules[bot]](https://github.com/google-labs-jules[bot])
- *(gateway)* pre-allocate routing vectors in proxy handlers ([#647](https://github.com/rolter-ai/rolter/pull/647)) by [@ormeilu](https://github.com/ormeilu)

### Testing
- *(gateway)* add edge case tests for multipart text_field ([#722](https://github.com/rolter-ai/rolter/pull/722)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add missing test for outbound_trace_headers [ROL-61] ([#718](https://github.com/rolter-ai/rolter/pull/718)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.10](https://github.com/rolter-ai/rolter/compare/v0.0.9...v0.0.10) - 2026-07-21

### Bug Fixes
- *(gateway)* gemini_native health probe uses x-goog-api-key ([#602](https://github.com/rolter-ai/rolter/pull/602)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(proxy)* add xai (grok) hosted provider kind ([#600](https://github.com/rolter-ai/rolter/pull/600)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add gemini/mistral/groq + native gemini generateContent kinds ([#598](https://github.com/rolter-ai/rolter/pull/598)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add versioned prompt templates and route decorators ([#594](https://github.com/rolter-ai/rolter/pull/594)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add custom guardrail webhook ([#593](https://github.com/rolter-ai/rolter/pull/593)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add built-in regex guardrails and PII redactor ([#592](https://github.com/rolter-ai/rolter/pull/592)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* resolve group-slug/model provider group addressing ([#572](https://github.com/rolter-ai/rolter/pull/572)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* route requests by bounded complexity tiers by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add complexity routing primitives by [@ormeilu](https://github.com/ormeilu)
- *(control)* persist advanced model config by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* implement medium-priority platform enhancements [ROL-65] ([#525](https://github.com/rolter-ai/rolter/pull/525)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add precise and LMCache-aware routing [ROL-54] ([#522](https://github.com/rolter-ai/rolter/pull/522)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add semantic response cache [ROL-57] ([#521](https://github.com/rolter-ai/rolter/pull/521)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add rotating egress proxy pools [ROL-101] ([#520](https://github.com/rolter-ai/rolter/pull/520)) by [@ormeilu](https://github.com/ormeilu)

### Testing
- *(gateway)* expand streaming retry and realtime coverage ([#524](https://github.com/rolter-ai/rolter/pull/524)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.9](https://github.com/rolter-ai/rolter/compare/v0.0.8...v0.0.9) - 2026-07-15

### Features
- *(gateway)* surface provider-slug/model ids in /v1/models ([#193](https://github.com/rolter-ai/rolter/pull/193)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* resolve provider-slug/model addressing with provider pinning ([#192](https://github.com/rolter-ai/rolter/pull/192)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* add immutable URL-safe provider slug for model addressing ([#191](https://github.com/rolter-ai/rolter/pull/191)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.8](https://github.com/ormeilu/rolter/compare/v0.0.7...v0.0.8) - 2026-07-13

### Bug Fixes
- *(gateway)* reject unsupported response resources ([#163](https://github.com/ormeilu/rolter/pull/163)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(proxy)* support custom ca bundles ([#168](https://github.com/ormeilu/rolter/pull/168)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* support responses lifecycle resources ([#166](https://github.com/ormeilu/rolter/pull/166)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* normalize provider role capabilities [ROL-262] ([#164](https://github.com/ormeilu/rolter/pull/164)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* runtime provider credentials, admin auth and gateway /admin proxy [ROL-250] ([#161](https://github.com/ormeilu/rolter/pull/161)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add responses api translation ([#162](https://github.com/ormeilu/rolter/pull/162)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* translate openai and anthropic APIs ([#159](https://github.com/ormeilu/rolter/pull/159)) by [@ormeilu](https://github.com/ormeilu)
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

### Testing
- *(gateway)* cover responses lifecycle target pinning ([#167](https://github.com/ormeilu/rolter/pull/167)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.7](https://github.com/ormeilu/rolter/compare/v0.0.6...v0.0.7) - 2026-07-12

### Features
- *(gateway)* hot-reload reliability tuning (breaker + health prober) ([#139](https://github.com/ormeilu/rolter/pull/139)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.6](https://github.com/ormeilu/rolter/compare/v0.0.5...v0.0.6) - 2026-07-12

### Bug Fixes
- *(gateway)* surface 4xx/5xx responses on terminal at info level ([#131](https://github.com/ormeilu/rolter/pull/131)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(gateway)* x-rolter-* routing-decision response headers ([#134](https://github.com/ormeilu/rolter/pull/134)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* fastest latency-aware routing strategy ([#130](https://github.com/ormeilu/rolter/pull/130)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* cheapest cost-aware routing strategy ([#128](https://github.com/ormeilu/rolter/pull/128)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* service-info landing on GET / ([#113](https://github.com/ormeilu/rolter/pull/113)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* served openapi document + embedded scalar api reference [ROL-72] ([#111](https://github.com/ormeilu/rolter/pull/111)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add /v1/audio/transcriptions + /v1/audio/translations ([#110](https://github.com/ormeilu/rolter/pull/110)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add /v1/audio/speech endpoint ([#109](https://github.com/ormeilu/rolter/pull/109)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add /v1/images/generations endpoint ([#108](https://github.com/ormeilu/rolter/pull/108)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add /v1/rerank endpoint ([#107](https://github.com/ormeilu/rolter/pull/107)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add /v1/embeddings endpoint ([#106](https://github.com/ormeilu/rolter/pull/106)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* opentelemetry otlp trace export via OTEL_* env [ROL-59] ([#104](https://github.com/ormeilu/rolter/pull/104)) by [@ormeilu](https://github.com/ormeilu)

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
