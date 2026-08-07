# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [0.0.11](https://github.com/rolter-ai/rolter/compare/rolter-control-v0.0.10...rolter-control-v0.0.11) - 2026-08-07

### Bug Fixes
- *(control)* enforce security_settings origins with a CORS layer [#813] ([#826](https://github.com/rolter-ai/rolter/pull/826)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* apply all five alerting code quality findings [#842] ([#842](https://github.com/rolter-ai/rolter/pull/842)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* guard the model and pricing catalog reads ([#792](https://github.com/rolter-ai/rolter/pull/792)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* surface queue_full on a shed request instead of a 502 [#639] ([#771](https://github.com/rolter-ai/rolter/pull/771)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* fix ClickHouse SQL parameter mismatch in MCP logs [ROL-SEC] ([#756](https://github.com/rolter-ai/rolter/pull/756)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* reconcile clickhouse log retention at startup ([#762](https://github.com/rolter-ai/rolter/pull/762)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* prevent timing attack in login handler ([#720](https://github.com/rolter-ai/rolter/pull/720)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* enforce slug validation during organization creation ([#721](https://github.com/rolter-ai/rolter/pull/721)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* make model price currency real and extensible ([#661](https://github.com/rolter-ai/rolter/pull/661)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* split the snapshot channel from the operator API ([#660](https://github.com/rolter-ai/rolter/pull/660)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* reject control characters in CRUD bodies with a 400 ([#658](https://github.com/rolter-ai/rolter/pull/658)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* deny link-local egress so a provider api_base can't reach metadata ([#655](https://github.com/rolter-ai/rolter/pull/655)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* omit unservable rows from the snapshot instead of 500ing ([#654](https://github.com/rolter-ai/rolter/pull/654)) by [@ormeilu](https://github.com/ormeilu)

### CI/CD
- lint rolter-control under the postgres feature ([#763](https://github.com/rolter-ai/rolter/pull/763)) by [@ormeilu](https://github.com/ormeilu)

### Dependencies
- *(deps)* bump p256 from 0.13.2 to 0.14.0 ([#800](https://github.com/rolter-ai/rolter/pull/800)) by [@dependabot[bot]](https://github.com/dependabot[bot])

### Features
- *(core)* add ROLTER_TELEMETRY_ENABLED as an explicit telemetry off [#812] ([#827](https://github.com/rolter-ai/rolter/pull/827)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* back the observability connectors screen with a real API [#511] ([#831](https://github.com/rolter-ai/rolter/pull/831)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* export logs, process and resource telemetry over OTLP [#809] ([#828](https://github.com/rolter-ai/rolter/pull/828)) by [@ormeilu](https://github.com/ormeilu)
- *(ui,control)* dashboard telemetry — browser tracing, runtime config, UX event ingest and emitters [#805] ([#811](https://github.com/rolter-ai/rolter/pull/811)) by [@ormeilu](https://github.com/ormeilu)
- *(ui,control)* build client and model settings screens [#564] ([#804](https://github.com/rolter-ai/rolter/pull/804)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* enforce access-profile model and route policy [#791] ([#803](https://github.com/rolter-ai/rolter/pull/803)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add SCIM Groups provisioning and group role mapping [#540] ([#788](https://github.com/rolter-ai/rolter/pull/788)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* complete the MCP OAuth authorization-code, refresh and exchange flow [#707] ([#789](https://github.com/rolter-ai/rolter/pull/789)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add configurable RBAC custom roles and access profiles ([#790](https://github.com/rolter-ai/rolter/pull/790)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* complete the org-owned MCP registry and management screens [#561] ([#782](https://github.com/rolter-ai/rolter/pull/782)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* build guardrail management screens [#562] ([#778](https://github.com/rolter-ai/rolter/pull/778)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* build plugin management screen [#567] ([#779](https://github.com/rolter-ai/rolter/pull/779)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* enforce MCP OAuth authorization ([#773](https://github.com/rolter-ai/rolter/pull/773)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* expose adaptive routing telemetry to the control plane ([#765](https://github.com/rolter-ai/rolter/pull/765)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* onboard accounts with one-time invitation links ([#713](https://github.com/rolter-ai/rolter/pull/713)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add optional oidc single sign-on ([#711](https://github.com/rolter-ai/rolter/pull/711)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add scim 2.0 user provisioning ([#708](https://github.com/rolter-ai/rolter/pull/708)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* persist and revoke mcp oauth grants and sessions ([#706](https://github.com/rolter-ai/rolter/pull/706)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* serve the rbac capability matrix from the server ([#703](https://github.com/rolter-ai/rolter/pull/703)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* persist and govern the adaptive routing policy ([#702](https://github.com/rolter-ai/rolter/pull/702)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add adaptive routing strategy ([#701](https://github.com/rolter-ai/rolter/pull/701)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* drain nodes out of service safely ([#700](https://github.com/rolter-ai/rolter/pull/700)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add cluster node inventory ([#699](https://github.com/rolter-ai/rolter/pull/699)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* manage request-log retention ([#698](https://github.com/rolter-ai/rolter/pull/698)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* report unavailable feature flags ([#697](https://github.com/rolter-ai/rolter/pull/697)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* manage cross-dialect compatibility policy ([#696](https://github.com/rolter-ai/rolter/pull/696)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* cap spend by business unit and customer ([#695](https://github.com/rolter-ai/rolter/pull/695)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* log business unit and customer attribution ([#689](https://github.com/rolter-ai/rolter/pull/689)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* attribute virtual-key spend to business units ([#688](https://github.com/rolter-ai/rolter/pull/688)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* register Gemini Interactions provider kind by [@ormeilu](https://github.com/ormeilu)
- *(control)* persist global runtime policy settings by [@ormeilu](https://github.com/ormeilu)
- *(control)* add skills repository CRUD APIs by [@ormeilu](https://github.com/ormeilu)
- *(control)* seed prompt template bootstrap data by [@ormeilu](https://github.com/ormeilu)
- *(control)* add prompt template CRUD and publish APIs by [@ormeilu](https://github.com/ormeilu)
- *(control)* add business unit and customer CRUD foundation by [@ormeilu](https://github.com/ormeilu)
- *(core)* expand provider adapter kind coverage [ROL-132] ([#645](https://github.com/rolter-ai/rolter/pull/645)) by [@ormeilu](https://github.com/ormeilu)

### Miscellaneous
- *(control)* align MCP transport allowlists across control plane and schema ([#794](https://github.com/rolter-ai/rolter/pull/794)) by [@ormeilu](https://github.com/ormeilu)

### Performance
- *(control)* fetch alert rules in a single query ([#729](https://github.com/rolter-ai/rolter/pull/729)) by [@ormeilu](https://github.com/ormeilu)

### Refactoring
- *(control)* derive authorize() requirements from the rbac capability table ([#769](https://github.com/rolter-ai/rolter/pull/769)) by [@ormeilu](https://github.com/ormeilu)

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
