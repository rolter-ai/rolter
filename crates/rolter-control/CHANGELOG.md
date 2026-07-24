# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [0.0.11](https://github.com/rolter-ai/rolter/compare/rolter-control-v0.0.10...rolter-control-v0.0.11) - 2026-07-24

### Features
- *(core)* expand provider adapter kind coverage [ROL-132] ([#645](https://github.com/rolter-ai/rolter/pull/645)) by [@ormeilu](https://github.com/ormeilu)

### Testing
- *(control)* exhaustive crate-level RBAC authorization matrix ([#638](https://github.com/rolter-ai/rolter/pull/638)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.10](https://github.com/rolter-ai/rolter/compare/rolter-control-v0.0.9...rolter-control-v0.0.10) - 2026-07-21

### Features
- *(proxy)* add xai (grok) hosted provider kind ([#600](https://github.com/rolter-ai/rolter/pull/600)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add gemini/mistral/groq + native gemini generateContent kinds ([#598](https://github.com/rolter-ai/rolter/pull/598)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* provider-group CRUD and provider_groups.default seed ([#582](https://github.com/rolter-ai/rolter/pull/582)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* seed providers.default into the DB once at startup ([#580](https://github.com/rolter-ai/rolter/pull/580)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* uniform readonly/default tier wrapper for providers and groups ([#579](https://github.com/rolter-ai/rolter/pull/579)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* ingest MCP tool-call logs ([#557](https://github.com/rolter-ai/rolter/pull/557)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* manage complexity routing policies by [@ormeilu](https://github.com/ormeilu)
- *(control)* manage complexity routing policies by [@ormeilu](https://github.com/ormeilu)
- *(control)* persist advanced model config by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* implement medium-priority platform enhancements [ROL-65] ([#525](https://github.com/rolter-ai/rolter/pull/525)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add precise and LMCache-aware routing [ROL-54] ([#522](https://github.com/rolter-ai/rolter/pull/522)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add rotating egress proxy pools [ROL-101] ([#520](https://github.com/rolter-ai/rolter/pull/520)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* record audit-log writes and surface them in the dashboard ([#500](https://github.com/rolter-ai/rolter/pull/500)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* reverse-proxy /gw/* to the gateway for the Playground [#493] ([#497](https://github.com/rolter-ai/rolter/pull/497)) by [@ormeilu](https://github.com/ormeilu)

### Other
- Merge pull request #553 from rolter-ai/feat/510-alerting-control-plane by [@ormeilu](https://github.com/ormeilu)
- Merge pull request #555 from rolter-ai/feat/536-audit-log-pagination-rebased by [@ormeilu](https://github.com/ormeilu)
- Merge pull request #554 from rolter-ai/feat/542-complexity-routing-policies by [@ormeilu](https://github.com/ormeilu)

### Testing
- *(control)* isolate integration tests per-schema to fix coverage race ([#604](https://github.com/rolter-ai/rolter/pull/604)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add exact format tests for session token generation ([#589](https://github.com/rolter-ai/rolter/pull/589)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add tests for generate_virtual_key ([#588](https://github.com/rolter-ai/rolter/pull/588)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.9](https://github.com/rolter-ai/rolter/compare/rolter-control-v0.0.8...rolter-control-v0.0.9) - 2026-07-15

### Features
- *(control)* self-service virtual keys + usage API [ROL-224] ([#198](https://github.com/rolter-ai/rolter/pull/198)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add user & membership CRUD API [ROL-223] ([#196](https://github.com/rolter-ai/rolter/pull/196)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* add immutable URL-safe provider slug for model addressing ([#191](https://github.com/rolter-ai/rolter/pull/191)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add per-invocation log viewer to Logs page ([#189](https://github.com/rolter-ai/rolter/pull/189)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* enforce per-user roles on control mutations (RBAC) ([#188](https://github.com/rolter-ai/rolter/pull/188)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add local account login/session auth (argon2id + postgres bearer tokens) ([#187](https://github.com/rolter-ai/rolter/pull/187)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.8](https://github.com/ormeilu/rolter/compare/rolter-control-v0.0.7...rolter-control-v0.0.8) - 2026-07-13

### Features
- *(proxy)* support custom ca bundles ([#168](https://github.com/ormeilu/rolter/pull/168)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* normalize provider role capabilities [ROL-262] ([#164](https://github.com/ormeilu/rolter/pull/164)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* runtime provider credentials, admin auth and gateway /admin proxy [ROL-250] ([#161](https://github.com/ormeilu/rolter/pull/161)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add cloud provider health adapters ([#157](https://github.com/ormeilu/rolter/pull/157)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add TEI embeddings provider ([#154](https://github.com/ormeilu/rolter/pull/154)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add OpenRouter provider ([#153](https://github.com/ormeilu/rolter/pull/153)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add self-hosted ollama provider ([#150](https://github.com/ormeilu/rolter/pull/150)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* CRUD API for per-virtual-key cache override ([#147](https://github.com/ormeilu/rolter/pull/147)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.6](https://github.com/ormeilu/rolter/compare/rolter-control-v0.0.5...rolter-control-v0.0.6) - 2026-07-12

### Dependencies
- *(deps)* bump rand from 0.8.6 to 0.10.2 ([#125](https://github.com/ormeilu/rolter/pull/125)) by [@dependabot[bot]](https://github.com/dependabot[bot])

### Features
- *(balancer)* fastest latency-aware routing strategy ([#130](https://github.com/ormeilu/rolter/pull/130)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* cheapest cost-aware routing strategy ([#128](https://github.com/ormeilu/rolter/pull/128)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* opentelemetry otlp trace export via OTEL_* env [ROL-59] ([#104](https://github.com/ormeilu/rolter/pull/104)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add rolter easy-up one-command bring-up ([#101](https://github.com/ormeilu/rolter/pull/101)) by [@ormeilu](https://github.com/ormeilu)

## [0.0.5](https://github.com/ormeilu/rolter/compare/rolter-control-v0.0.4...rolter-control-v0.0.5) - 2026-07-11

### Added

- *(control)* uptime %/MTTR/timeline rollup api over provider_health_events ([#87](https://github.com/ormeilu/rolter/pull/87))

### Other

- *(control)* postgres-backed CRUD + snapshot integration tests, run in CI ([#92](https://github.com/ormeilu/rolter/pull/92))

## [0.0.4](https://github.com/ormeilu/rolter/compare/rolter-control-v0.0.3...rolter-control-v0.0.4) - 2026-07-10

### Added

- *(store)* DB-defined per-model param defaults + override policy ([#71](https://github.com/ormeilu/rolter/pull/71))
- *(balancer)* wire the scorer pipeline in as a selectable `pipeline` strategy ([#59](https://github.com/ormeilu/rolter/pull/59))

### Other

- taplo-format all TOML + make taplo check blocking [ROL-124] ([#69](https://github.com/ormeilu/rolter/pull/69))
- expand quality gate into a hardened multi-check pipeline [ROL-124] ([#54](https://github.com/ormeilu/rolter/pull/54))

## [0.0.2](https://github.com/ormeilu/rolter/compare/v0.0.1...v0.0.2) - 2026-07-02

### Added

- *(control)* split config vs DB models, LiteLLM-style ([#17](https://github.com/ormeilu/rolter/pull/17))
- *(control)* add CRUD API for orgs/teams/projects/providers/routes/keys ([#13](https://github.com/ormeilu/rolter/pull/13))
- *(control)* add rolter-seed bootstrap CLI ([#12](https://github.com/ormeilu/rolter/pull/12))
- *(control)* serve versioned config snapshots for gateway polling ([#11](https://github.com/ormeilu/rolter/pull/11))
- *(core)* scaffold rolter workspace and runnable gateway mvp

### Other

- release v0.0.1 ([#3](https://github.com/ormeilu/rolter/pull/3))

## [0.0.1](https://github.com/ormeilu/rolter/releases/tag/v0.0.1) - 2026-06-30

### Added

- *(core)* scaffold rolter workspace and runnable gateway mvp
