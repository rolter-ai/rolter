# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
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
