# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [0.1.1](https://github.com/rolter-ai/rolter/compare/rolter-v0.1.0...rolter-v0.1.1) - 2026-09-11

### Bug Fixes
- *(gateway)* give the fleet one place to decide where request logs go [#929] ([#1174](https://github.com/rolter-ai/rolter/pull/1174)) by [@ormeilu](https://github.com/ormeilu)

### Build
- *(control)* fix easy_up Args literal for rolter-control/postgres [#1295] ([#1305](https://github.com/rolter-ai/rolter/pull/1305)) by [@ormeilu](https://github.com/ormeilu)

### Documentation
- record what 1.0.0 guarantees on each api surface ([#1427](https://github.com/rolter-ai/rolter/pull/1427)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(core)* warn about unrecognised rolter.toml keys at startup ([#1438](https://github.com/rolter-ai/rolter/pull/1438)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* report unrecognised rolter.toml keys in rolter check ([#1433](https://github.com/rolter-ai/rolter/pull/1433)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* totp second factor for local accounts [#1078] ([#1324](https://github.com/rolter-ai/rolter/pull/1324)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* export the live configuration as importable rolter.toml [#1082] ([#1311](https://github.com/rolter-ai/rolter/pull/1311)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* cached update check with a dashboard hint and a cli notice [#902] ([#1294](https://github.com/rolter-ai/rolter/pull/1294)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* per-budget override for unpriced_policy [#996] ([#1286](https://github.com/rolter-ai/rolter/pull/1286)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* verify and rotate the KEK against the control-plane store [#923] ([#1175](https://github.com/rolter-ai/rolter/pull/1175)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* throttle and audit failed logins [#1079] ([#1161](https://github.com/rolter-ai/rolter/pull/1161)) by [@ormeilu](https://github.com/ormeilu)

### Refactoring
- *(control)* resolve the public base url once, not per request ([#1435](https://github.com/rolter-ai/rolter/pull/1435)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.11](https://github.com/rolter-ai/rolter/compare/rolter-v0.0.10...rolter-v0.0.11) - 2026-08-13

### Bug Fixes
- *(ui)* drive the currency chooser from the configured rate table ([#978](https://github.com/rolter-ai/rolter/pull/978)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* refuse open mode on a non-loopback bind [#970] ([#971](https://github.com/rolter-ai/rolter/pull/971)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* drop the inert guardrail default_on flag ([#759](https://github.com/rolter-ai/rolter/pull/759)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* make model price currency real and extensible ([#661](https://github.com/rolter-ai/rolter/pull/661)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* split the snapshot channel from the operator API ([#660](https://github.com/rolter-ai/rolter/pull/660)) by [@ormeilu](https://github.com/ormeilu)

### Documentation
- *(deployment)* document the egress destination policy ([#760](https://github.com/rolter-ai/rolter/pull/760)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(gateway)* make unpriced traffic an explicit budget policy ([#997](https://github.com/rolter-ai/rolter/pull/997)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* external PII sanitizer adapter with opt-in restoration ([#910](https://github.com/rolter-ai/rolter/pull/910)) by [@ormeilu](https://github.com/ormeilu)
- *(cli)* guided secure configuration for production deploys ([#913](https://github.com/rolter-ai/rolter/pull/913)) by [@ormeilu](https://github.com/ormeilu)
- add rolter check pre-boot validation for production [#854] ([#883](https://github.com/rolter-ai/rolter/pull/883)) by [@ormeilu](https://github.com/ormeilu)
- *(ui,control)* dashboard telemetry — browser tracing, runtime config, UX event ingest and emitters [#805] ([#811](https://github.com/rolter-ai/rolter/pull/811)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* support gemini interactions api [#599] ([#761](https://github.com/rolter-ai/rolter/pull/761)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* enforce post_call guardrails on non-streaming responses ([#667](https://github.com/rolter-ai/rolter/pull/667)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* per-route guardrail enable/disable overrides ([#664](https://github.com/rolter-ai/rolter/pull/664)) by [@ormeilu](https://github.com/ormeilu)

### Miscellaneous
- repoint stale ormeilu/rolter urls at the rolter-ai org [#873] ([#875](https://github.com/rolter-ai/rolter/pull/875)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.10](https://github.com/rolter-ai/rolter/compare/rolter-v0.0.9...rolter-v0.0.10) - 2026-07-21

### Features
- *(proxy)* add xai (grok) hosted provider kind ([#600](https://github.com/rolter-ai/rolter/pull/600)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add gemini/mistral/groq + native gemini generateContent kinds ([#598](https://github.com/rolter-ai/rolter/pull/598)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add versioned prompt templates and route decorators ([#594](https://github.com/rolter-ai/rolter/pull/594)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add custom guardrail webhook ([#593](https://github.com/rolter-ai/rolter/pull/593)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add built-in regex guardrails and PII redactor ([#592](https://github.com/rolter-ai/rolter/pull/592)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* uniform readonly/default tier wrapper for providers and groups ([#579](https://github.com/rolter-ai/rolter/pull/579)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* resolve group-slug/model provider group addressing ([#572](https://github.com/rolter-ai/rolter/pull/572)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* implement medium-priority platform enhancements [ROL-65] ([#525](https://github.com/rolter-ai/rolter/pull/525)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add precise and LMCache-aware routing [ROL-54] ([#522](https://github.com/rolter-ai/rolter/pull/522)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add semantic response cache [ROL-57] ([#521](https://github.com/rolter-ai/rolter/pull/521)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add rotating egress proxy pools [ROL-101] ([#520](https://github.com/rolter-ai/rolter/pull/520)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* reverse-proxy /gw/* to the gateway for the Playground [#493] ([#497](https://github.com/rolter-ai/rolter/pull/497)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.9](https://github.com/rolter-ai/rolter/compare/rolter-v0.0.8...rolter-v0.0.9) - 2026-07-15

### Miscellaneous
- update Cargo.lock dependencies
## [0.0.8](https://github.com/ormeilu/rolter/compare/rolter-v0.0.7...rolter-v0.0.8) - 2026-07-13

### Features
- *(proxy)* support custom ca bundles ([#168](https://github.com/ormeilu/rolter/pull/168)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* normalize provider role capabilities [ROL-262] ([#164](https://github.com/ormeilu/rolter/pull/164)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* runtime provider credentials, admin auth and gateway /admin proxy [ROL-250] ([#161](https://github.com/ormeilu/rolter/pull/161)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* isolate provider queues and backpressure ([#158](https://github.com/ormeilu/rolter/pull/158)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add cloud provider health adapters ([#157](https://github.com/ormeilu/rolter/pull/157)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add OpenRouter provider ([#153](https://github.com/ormeilu/rolter/pull/153)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add self-hosted ollama provider ([#150](https://github.com/ormeilu/rolter/pull/150)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* configurable request body-size limit ([#148](https://github.com/ormeilu/rolter/pull/148)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* per-virtual-key response-cache override ([#146](https://github.com/ormeilu/rolter/pull/146)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* cache streaming/SSE responses ([#145](https://github.com/ormeilu/rolter/pull/145)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* exact-match response cache (redis, ttl, per-route opt-in) ([#142](https://github.com/ormeilu/rolter/pull/142)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.6](https://github.com/ormeilu/rolter/compare/rolter-v0.0.5...rolter-v0.0.6) - 2026-07-12

### Features
- *(core)* opentelemetry otlp trace export via OTEL_* env [ROL-59] ([#104](https://github.com/ormeilu/rolter/pull/104)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add rolter easy-up one-command bring-up ([#101](https://github.com/ormeilu/rolter/pull/101)) by [@ormeilu](https://github.com/ormeilu)
