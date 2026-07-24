# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [0.0.11](https://github.com/rolter-ai/rolter/compare/rolter-store-v0.0.10...rolter-store-v0.0.11) - 2026-07-24

### Bug Fixes
- *(store)* decode budgets.limit_usd as text in snapshot load ([#628](https://github.com/rolter-ai/rolter/pull/628)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(core)* expand provider adapter kind coverage [ROL-132] ([#645](https://github.com/rolter-ai/rolter/pull/645)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.10](https://github.com/rolter-ai/rolter/compare/rolter-store-v0.0.9...rolter-store-v0.0.10) - 2026-07-21

### Features
- *(proxy)* add xai (grok) hosted provider kind ([#600](https://github.com/rolter-ai/rolter/pull/600)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add gemini/mistral/groq + native gemini generateContent kinds ([#598](https://github.com/rolter-ai/rolter/pull/598)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* provider_groups tables, repo, and merge wiring ([#581](https://github.com/rolter-ai/rolter/pull/581)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* paginate and filter audit logs by [@ormeilu](https://github.com/ormeilu)
- *(control)* persist advanced model config by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* implement medium-priority platform enhancements [ROL-65] ([#525](https://github.com/rolter-ai/rolter/pull/525)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add precise and LMCache-aware routing [ROL-54] ([#522](https://github.com/rolter-ai/rolter/pull/522)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add rotating egress proxy pools [ROL-101] ([#520](https://github.com/rolter-ai/rolter/pull/520)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* record audit-log writes and surface them in the dashboard ([#500](https://github.com/rolter-ai/rolter/pull/500)) by [@ormeilu](https://github.com/ormeilu)

### Other
- Merge pull request #553 from rolter-ai/feat/510-alerting-control-plane by [@ormeilu](https://github.com/ormeilu)
- Merge pull request #549 from rolter-ai/feat/533-security-settings-policy by [@ormeilu](https://github.com/ormeilu)
- Merge pull request #547 from rolter-ai/feat/532-advanced-model-config by [@ormeilu](https://github.com/ormeilu)

### Testing
- *(control)* isolate integration tests per-schema to fix coverage race ([#604](https://github.com/rolter-ai/rolter/pull/604)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.9](https://github.com/rolter-ai/rolter/compare/rolter-store-v0.0.8...rolter-store-v0.0.9) - 2026-07-15

### Dependencies
- *(deps)* bump aes-gcm from 0.10.3 to 0.11.0 ([#183](https://github.com/rolter-ai/rolter/pull/183)) by [@dependabot[bot]](https://github.com/dependabot[bot])

### Features
- *(control)* self-service virtual keys + usage API [ROL-224] ([#198](https://github.com/rolter-ai/rolter/pull/198)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add user & membership CRUD API [ROL-223] ([#196](https://github.com/rolter-ai/rolter/pull/196)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* add immutable URL-safe provider slug for model addressing ([#191](https://github.com/rolter-ai/rolter/pull/191)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* enforce per-user roles on control mutations (RBAC) ([#188](https://github.com/rolter-ai/rolter/pull/188)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add local account login/session auth (argon2id + postgres bearer tokens) ([#187](https://github.com/rolter-ai/rolter/pull/187)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.8](https://github.com/ormeilu/rolter/compare/rolter-store-v0.0.7...rolter-store-v0.0.8) - 2026-07-13

### Features
- *(proxy)* support custom ca bundles ([#168](https://github.com/ormeilu/rolter/pull/168)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* normalize provider role capabilities [ROL-262] ([#164](https://github.com/ormeilu/rolter/pull/164)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* runtime provider credentials, admin auth and gateway /admin proxy [ROL-250] ([#161](https://github.com/ormeilu/rolter/pull/161)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add cloud provider health adapters ([#157](https://github.com/ormeilu/rolter/pull/157)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add TEI embeddings provider ([#154](https://github.com/ormeilu/rolter/pull/154)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add OpenRouter provider ([#153](https://github.com/ormeilu/rolter/pull/153)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add self-hosted ollama provider ([#150](https://github.com/ormeilu/rolter/pull/150)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* CRUD API for per-virtual-key cache override ([#147](https://github.com/ormeilu/rolter/pull/147)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* per-virtual-key response-cache override ([#146](https://github.com/ormeilu/rolter/pull/146)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* exact-match response cache (redis, ttl, per-route opt-in) ([#142](https://github.com/ormeilu/rolter/pull/142)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.6](https://github.com/ormeilu/rolter/compare/rolter-store-v0.0.5...rolter-store-v0.0.6) - 2026-07-12

### Features
- *(balancer)* fastest latency-aware routing strategy ([#130](https://github.com/ormeilu/rolter/pull/130)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* cheapest cost-aware routing strategy ([#128](https://github.com/ormeilu/rolter/pull/128)) by [@ormeilu](https://github.com/ormeilu)

## [0.0.5](https://github.com/ormeilu/rolter/compare/rolter-store-v0.0.4...rolter-store-v0.0.5) - 2026-07-11

### Added

- *(gateway)* provider status-page secondary health signal [ROL-200] ([#90](https://github.com/ormeilu/rolter/pull/90))
- *(gateway)* opt-in also_track_via_llm_call end-to-end health check ([#89](https://github.com/ormeilu/rolter/pull/89))
- *(core)* multiple weighted api keys per provider ([#83](https://github.com/ormeilu/rolter/pull/83))

## [0.0.4](https://github.com/ormeilu/rolter/compare/rolter-store-v0.0.3...rolter-store-v0.0.4) - 2026-07-10

### Added

- *(store)* DB-defined per-model param defaults + override policy ([#71](https://github.com/ormeilu/rolter/pull/71))
- *(core)* weighted variant abstraction with ordered fallback ([#65](https://github.com/ormeilu/rolter/pull/65))
- *(gateway)* per-model default inference params with admin override policy ([#61](https://github.com/ormeilu/rolter/pull/61))
- *(balancer)* wire the scorer pipeline in as a selectable `pipeline` strategy ([#59](https://github.com/ormeilu/rolter/pull/59))
- *(balancer)* weighted selection honoring Target.weight [ROL-51] ([#50](https://github.com/ormeilu/rolter/pull/50))
- *(gateway)* rpm/tpm rate limits with redis sliding window ([#42](https://github.com/ormeilu/rolter/pull/42))

### Fixed

- *(store)* package migrations inside rolter-store so the published crate builds ([#70](https://github.com/ormeilu/rolter/pull/70))

## [0.0.3](https://github.com/ormeilu/rolter/compare/rolter-store-v0.0.2...rolter-store-v0.0.3) - 2026-07-09

### Added

- *(gateway)* budget enforcement with redis spend counters ([#37](https://github.com/ormeilu/rolter/pull/37))

## [0.0.2](https://github.com/ormeilu/rolter/compare/v0.0.1...v0.0.2) - 2026-07-02

### Added

- *(control)* split config vs DB models, LiteLLM-style ([#17](https://github.com/ormeilu/rolter/pull/17))
- *(control)* serve versioned config snapshots for gateway polling ([#11](https://github.com/ormeilu/rolter/pull/11))
- *(store)* add postgres repository layer for tenancy/routing/limits ([#10](https://github.com/ormeilu/rolter/pull/10))
- *(store)* postgres-backed ConfigStore with sqlx migration runner [ROL-20 ROL-21] ([#9](https://github.com/ormeilu/rolter/pull/9))
- *(core)* scaffold rolter workspace and runnable gateway mvp

### Other

- release v0.0.1 ([#3](https://github.com/ormeilu/rolter/pull/3))

## [0.0.1](https://github.com/ormeilu/rolter/releases/tag/v0.0.1) - 2026-06-30

### Added

- *(core)* scaffold rolter workspace and runnable gateway mvp
