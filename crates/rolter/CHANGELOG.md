# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [1.0.0](https://github.com/rolter-ai/rolter/compare/rolter-v0.1.0...rolter-v1.0.0) - 2026-10-31

### Bug Fixes
- *(gateway)* harden local classifier loading and scoring [#3511] ([#3574](https://github.com/rolter-ai/rolter/pull/3574)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* narrowed org rows fall through to global [#3546] ([#3572](https://github.com/rolter-ai/rolter/pull/3572)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* key_hash out of responses, org model labels [#3525] ([#3560](https://github.com/rolter-ai/rolter/pull/3560)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* forward only an mcp body the relay has judged [#3527] ([#3551](https://github.com/rolter-ai/rolter/pull/3551)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* control-plane operations and auth follow-ups [#3477] ([#3536](https://github.com/rolter-ai/rolter/pull/3536)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* apply the dialled-targets rule everywhere [#3487] ([#3515](https://github.com/rolter-ai/rolter/pull/3515)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* routing checks and provider profiles [#3430] ([#3503](https://github.com/rolter-ai/rolter/pull/3503)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* control-plane operations follow-ups [#3433] ([#3489](https://github.com/rolter-ai/rolter/pull/3489)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* give a zero-weight variant no first attempts [#3341] ([#3447](https://github.com/rolter-ai/rolter/pull/3447)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* analytics and alerting follow-ups [#3294] ([#3442](https://github.com/rolter-ai/rolter/pull/3442)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* nine small control, cli and docs fixes [#3221, #3219, #3218, #1995, #1946, #1888, #2768, #1720, #1405] ([#3309](https://github.com/rolter-ai/rolter/pull/3309)) by [@ormeilu](https://github.com/ormeilu)
- close ten more post-1.0 polish issues [#1133, #3147, #1916] ([#3215](https://github.com/rolter-ai/rolter/pull/3215)) by [@ormeilu](https://github.com/ormeilu)
- *(docs)* document nested config structs and guard drift in tests ([#3117](https://github.com/rolter-ai/rolter/pull/3117)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* [**breaking**] deliver alerts outside the lock, bound api writes, refuse short admin tokens, warn on a public dev key [#1967, #1925, #1964, #1965] ([#3146](https://github.com/rolter-ai/rolter/pull/3146)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* keep the snapshot serving when a provider is left out [#2950, #2946, #2947] ([#3041](https://github.com/rolter-ai/rolter/pull/3041)) by [@ormeilu](https://github.com/ormeilu)
- *(infra)* align exposure wording with the admin-token preflight rule [#2975] ([#3007](https://github.com/rolter-ai/rolter/pull/3007)) by [@ormeilu](https://github.com/ormeilu)
- *(infra)* let a default helm install pass its own strict preflight [#1939] ([#2974](https://github.com/rolter-ai/rolter/pull/2974)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* apply the configured request id header [#2925] ([#2967](https://github.com/rolter-ai/rolter/pull/2967)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* apply or retire route advanced settings that were never read [#2924] ([#2942](https://github.com/rolter-ai/rolter/pull/2942)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* reject negative and NaN price rates, retire the unread route cache-write field [#2889, #2898, #2890] ([#2900](https://github.com/rolter-ai/rolter/pull/2900)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* hide secret env values in --help output [#2793] ([#2821](https://github.com/rolter-ai/rolter/pull/2821)) by [@ormeilu](https://github.com/ormeilu)
- *(infra)* record a failed release check so the launcher waits a day before retrying ([#2612](https://github.com/rolter-ai/rolter/pull/2612)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* keep the public sk-rolter-dev key off production paths ([#2481](https://github.com/rolter-ai/rolter/pull/2481)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* keep datastore credentials out of startup logs ([#2460](https://github.com/rolter-ai/rolter/pull/2460)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* remove the unenforced dashboard password setting ([#2458](https://github.com/rolter-ai/rolter/pull/2458)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* exit easy-up on ctrl-c and sigterm and drain the control plane ([#2380](https://github.com/rolter-ai/rolter/pull/2380)) by [@ormeilu](https://github.com/ormeilu)
- *(infra)* bind the published image to every interface behind the open-mode guard [#1891] ([#1950](https://github.com/rolter-ai/rolter/pull/1950)) by [@ormeilu](https://github.com/ormeilu)
- *(docs)* add missing VirtualKeyConfig fields to config reference ([#1955](https://github.com/rolter-ai/rolter/pull/1955)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* meter realtime sessions against budgets and rate limits [#1396] ([#1913](https://github.com/rolter-ai/rolter/pull/1913)) by [@ormeilu](https://github.com/ormeilu)
- generate and check the session pepper in rolter init and check ([#1948](https://github.com/rolter-ai/rolter/pull/1948)) by [@ormeilu](https://github.com/ormeilu)
- *(docs)* document missing runtime environment variables in reference docs ([#1870](https://github.com/rolter-ai/rolter/pull/1870)) by [@ormeilu](https://github.com/ormeilu)
- *(infra)* set ROLTER_REDIS_URL in the chart and name the prefixed env vars [#1771] ([#1784](https://github.com/rolter-ai/rolter/pull/1784)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* make the ui_events opt-out reachable in a postgres deployment [#1748] ([#1767](https://github.com/rolter-ai/rolter/pull/1767)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* partition semantic cache by response contract [#1476] ([#1768](https://github.com/rolter-ai/rolter/pull/1768)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* print a try-it command easy-up's own config accepts ([#1703](https://github.com/rolter-ai/rolter/pull/1703)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* give the fleet one place to decide where request logs go [#929] ([#1174](https://github.com/rolter-ai/rolter/pull/1174)) by [@ormeilu](https://github.com/ormeilu)

### Build
- debian13 runtime base, cargo auditable images, rolter seed [#1897, #2702, #2747] ([#3230](https://github.com/rolter-ai/rolter/pull/3230)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* fix easy_up Args literal for rolter-control/postgres [#1295] ([#1305](https://github.com/rolter-ai/rolter/pull/1305)) by [@ormeilu](https://github.com/ormeilu)

### Documentation
- *(docs)* move the user docs from mintlify to docs7 [#3223] ([#3224](https://github.com/rolter-ai/rolter/pull/3224)) by [@ormeilu](https://github.com/ormeilu)
- document signal-specific OTLP environment variables ([#3178](https://github.com/rolter-ai/rolter/pull/3178)) by [@ormeilu](https://github.com/ormeilu)
- add OPENROUTER_HTTP_REFERER and OPENROUTER_X_TITLE to reference ([#3001](https://github.com/rolter-ai/rolter/pull/3001)) by [@ormeilu](https://github.com/ormeilu)
- state the ROLTER_TEST_ rule for test-only env vars [#2772] ([#2966](https://github.com/rolter-ai/rolter/pull/2966)) by [@ormeilu](https://github.com/ormeilu)
- *(docs)* complete the rolter.toml reference and scope its drift guard [#2920] ([#2932](https://github.com/rolter-ai/rolter/pull/2932)) by [@ormeilu](https://github.com/ormeilu)
- *(docs)* document missing ProviderConfig fields in config reference ([#2500](https://github.com/rolter-ai/rolter/pull/2500)) by [@ormeilu](https://github.com/ormeilu)
- *(docs)* document CacheConfig fields in config reference ([#2764](https://github.com/rolter-ai/rolter/pull/2764)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* document the file-configurable [security] keys [#2551] ([#2634](https://github.com/rolter-ai/rolter/pull/2634)) by [@ormeilu](https://github.com/ormeilu)
- *(docs)* document the four undocumented [health] keys ([#2394](https://github.com/rolter-ai/rolter/pull/2394)) by [@ormeilu](https://github.com/ormeilu)
- add require_auth to example toml and guard config fields ([#1958](https://github.com/rolter-ai/rolter/pull/1958)) by [@ormeilu](https://github.com/ormeilu)
- *(docs)* add missing ServerConfig fields to config reference ([#1812](https://github.com/rolter-ai/rolter/pull/1812)) by [@ormeilu](https://github.com/ormeilu)
- list multi-region and prompt-caching pages in summary ([#1794](https://github.com/rolter-ai/rolter/pull/1794)) by [@ormeilu](https://github.com/ormeilu)
- document missing binary environment variables ([#1809](https://github.com/rolter-ai/rolter/pull/1809)) by [@ormeilu](https://github.com/ormeilu)
- consolidate both doc trees under docs/ as user-docs and dev-docs [#1516] ([#1517](https://github.com/rolter-ai/rolter/pull/1517)) by [@ormeilu](https://github.com/ormeilu)
- record what 1.0.0 guarantees on each api surface ([#1427](https://github.com/rolter-ai/rolter/pull/1427)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(gateway)* run experimental onnx local classifiers [#3443] ([#3584](https://github.com/rolter-ai/rolter/pull/3584)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* config export follow-ups [#3481] ([#3530](https://github.com/rolter-ai/rolter/pull/3530)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* local classifier follow-ups [#3452] ([#3518](https://github.com/rolter-ai/rolter/pull/3518)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* export one organization per file and carry the mcp marker [#3440] ([#3492](https://github.com/rolter-ai/rolter/pull/3492)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* run semantic guard providers as in-process classifiers [#3329] ([#3456](https://github.com/rolter-ai/rolter/pull/3456)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* fallback failure classes and response guard follow-ups [#3319] ([#3437](https://github.com/rolter-ai/rolter/pull/3437)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* wave 4 control-plane ops follow-ups [#3311] ([#3417](https://github.com/rolter-ai/rolter/pull/3417)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* let a fallback target declare which failure classes it takes over [#1584] ([#3376](https://github.com/rolter-ai/rolter/pull/3376)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* scoped semantic guard policies over several guardrail providers [#459] ([#3371](https://github.com/rolter-ai/rolter/pull/3371)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* restore pii placeholders on streamed responses, capture the pre-restore body [#911, #3240] ([#3357](https://github.com/rolter-ai/rolter/pull/3357)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* mcp tool discovery, enforced tool groups and org settings, oauth scope step-up, connection status [#1283, #1403, #1404, #1417, #951] ([#3318](https://github.com/rolter-ai/rolter/pull/3318)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* ldap sign-in, per-org mfa enrolment, admin-token sign-in and emailed invitations [#1826, #1909, #1828, #1590] ([#3313](https://github.com/rolter-ai/rolter/pull/3313)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* cap spend and rate per key at mint and per person, carried by config export [#1171, #1830, #1312, #3128] ([#3312](https://github.com/rolter-ai/rolter/pull/3312)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* redact secrets in captured payloads and erase a subject's logs on request [#1835, #1085] ([#3310](https://github.com/rolter-ai/rolter/pull/3310)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* independent model catalogue, /v1/models metadata and whole ids, deprecation registry [#1729, #2939, #3245, #1423] ([#3288](https://github.com/rolter-ai/rolter/pull/3288)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add an infinity embeddings and reranking provider kind [#463] ([#3260](https://github.com/rolter-ai/rolter/pull/3260)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* support the system one decision api [#3150, #2535] ([#3156](https://github.com/rolter-ai/rolter/pull/3156)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* price realtime audio and text tokens separately and meter input transcription [#1882, #1883] ([#3142](https://github.com/rolter-ai/rolter/pull/3142)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* price image and audio requests by unit [#2937] ([#3120](https://github.com/rolter-ai/rolter/pull/3120)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* per-route request, token and concurrency limits [#2936] ([#3110](https://github.com/rolter-ai/rolter/pull/3110)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* name guardrail providers in outcome events and log realtime plugin calls [#3038, #3071] ([#3086](https://github.com/rolter-ai/rolter/pull/3086)) by [@ormeilu](https://github.com/ormeilu)
- *(infra)* rolter init writes the compose datastore passwords [#2413, #2424] ([#3079](https://github.com/rolter-ai/rolter/pull/3079)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* price anthropic 1 hour cache writes at their own rate [#2891] ([#2901](https://github.com/rolter-ai/rolter/pull/2901)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* price tokens written to the prompt cache [#2876] ([#2892](https://github.com/rolter-ai/rolter/pull/2892)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* warn when a standalone gateway config sets project_scoped ([#2604](https://github.com/rolter-ai/rolter/pull/2604)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* let providers and groups be scoped to one project ([#2470](https://github.com/rolter-ai/rolter/pull/2470)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* record the struggle signals the ux stream cannot express [#1731] ([#1746](https://github.com/rolter-ai/rolter/pull/1746)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* give the dashboard a documentation base URL ([#1656](https://github.com/rolter-ai/rolter/pull/1656)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* migrate rolter.toml across schema versions on load [#326] ([#1515](https://github.com/rolter-ai/rolter/pull/1515)) by [@ormeilu](https://github.com/ormeilu)
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

### Testing
- generate the remaining literal test credentials at run time [#2685] ([#2718](https://github.com/rolter-ai/rolter/pull/2718)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* lint the vendored easy-up example config directly [#1440] ([#1471](https://github.com/rolter-ai/rolter/pull/1471)) by [@ormeilu](https://github.com/ormeilu)
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
