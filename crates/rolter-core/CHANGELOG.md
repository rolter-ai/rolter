# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [1.0.0](https://github.com/rolter-ai/rolter/compare/rolter-core-v0.1.0...rolter-core-v1.0.0) - 2026-11-01

### Bug Fixes
- *(gateway)* weight realtime primaries and scan variants once [#3553, #3561] ([#3600](https://github.com/rolter-ai/rolter/pull/3600)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* coalesce config problem sampling [#3565] ([#3597](https://github.com/rolter-ai/rolter/pull/3597)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* correct mcp outcome logging [#3579, #3564, #3562] ([#3594](https://github.com/rolter-ai/rolter/pull/3594)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* record mcp calls no reply was awaited for [#3529] ([#3573](https://github.com/rolter-ai/rolter/pull/3573)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* harden local classifier loading and scoring [#3511] ([#3574](https://github.com/rolter-ai/rolter/pull/3574)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* narrowed org rows fall through to global [#3546] ([#3572](https://github.com/rolter-ai/rolter/pull/3572)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* forward only an mcp body the relay has judged [#3527] ([#3551](https://github.com/rolter-ai/rolter/pull/3551)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* apply the dialled-targets rule everywhere [#3487] ([#3515](https://github.com/rolter-ai/rolter/pull/3515)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* routing checks and provider profiles [#3430] ([#3503](https://github.com/rolter-ai/rolter/pull/3503)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* gateway request path follow-ups [#3404] ([#3491](https://github.com/rolter-ai/rolter/pull/3491)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* per-org namespace follow-ups [#3459] ([#3490](https://github.com/rolter-ai/rolter/pull/3490)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* give a zero-weight variant no first attempts [#3341] ([#3447](https://github.com/rolter-ai/rolter/pull/3447)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* snapshot format follow-ups [#3347] ([#3438](https://github.com/rolter-ai/rolter/pull/3438)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* nine small control, cli and docs fixes [#3221, #3219, #3218, #1995, #1946, #1888, #2768, #1720, #1405] ([#3309](https://github.com/rolter-ai/rolter/pull/3309)) by [@ormeilu](https://github.com/ormeilu)
- close ten more post-1.0 polish issues [#1133, #3147, #1916] ([#3215](https://github.com/rolter-ai/rolter/pull/3215)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* [**breaking**] deliver alerts outside the lock, bound api writes, refuse short admin tokens, warn on a public dev key [#1967, #1925, #1964, #1965] ([#3146](https://github.com/rolter-ai/rolter/pull/3146)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* refuse secret env names as a provider credential when building the snapshot [#3097] ([#3109](https://github.com/rolter-ai/rolter/pull/3109)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* bound the shutdown drain and check guard chains against capabilities [#2378, #2377] ([#3087](https://github.com/rolter-ai/rolter/pull/3087)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* keep the snapshot serving when a provider is left out [#2950, #2946, #2947] ([#3041](https://github.com/rolter-ai/rolter/pull/3041)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* clear a denied status_page_url instead of dropping the provider [#3005] ([#3009](https://github.com/rolter-ai/rolter/pull/3009)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* strip a denied telemetry endpoint instead of dropping the provider [#2997] ([#3004](https://github.com/rolter-ai/rolter/pull/3004)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* check the request id header against the names the API refuses [#2970] ([#2999](https://github.com/rolter-ai/rolter/pull/2999)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* apply the configured request id header [#2925] ([#2967](https://github.com/rolter-ai/rolter/pull/2967)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* prune targets of an omitted provider instead of refusing the snapshot [#2945] ([#2949](https://github.com/rolter-ai/rolter/pull/2949)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* fail closed when security or egress settings do not parse [#2943] ([#2948](https://github.com/rolter-ai/rolter/pull/2948)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* apply or retire route advanced settings that were never read [#2924] ([#2942](https://github.com/rolter-ai/rolter/pull/2942)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* serve dashboard-owned tables from the store in merged mode [#2922] ([#2941](https://github.com/rolter-ai/rolter/pull/2941)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* take the single api key with pop instead of remove(0) [#2911] ([#2912](https://github.com/rolter-ai/rolter/pull/2912)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* reject negative and NaN price rates, retire the unread route cache-write field [#2889, #2898, #2890] ([#2900](https://github.com/rolter-ai/rolter/pull/2900)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* make the provider sheet's preview and hints follow the kind [#2811] ([#2840](https://github.com/rolter-ai/rolter/pull/2840)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* send the anthropic key as x-api-key in the provider connection test [#2806] ([#2827](https://github.com/rolter-ai/rolter/pull/2827)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* hide secret env values in --help output [#2793] ([#2821](https://github.com/rolter-ai/rolter/pull/2821)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* make a prompt template variable's required flag decide the refusal ([#2598](https://github.com/rolter-ai/rolter/pull/2598)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* treat an empty guardrail replacement as the default token ([#2569](https://github.com/rolter-ai/rolter/pull/2569)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* compare route guardrail overrides with the trimmed rule name ([#2548](https://github.com/rolter-ai/rolter/pull/2548)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* remove the virtual-key switch that never changed a decision ([#2550](https://github.com/rolter-ai/rolter/pull/2550)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* apply the egress policy to every operator-written url [#2383] ([#2492](https://github.com/rolter-ai/rolter/pull/2492)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* refuse a realtime session under a fail-closed PII sanitizer ([#2498](https://github.com/rolter-ai/rolter/pull/2498)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* scan function-call arguments on realtime sessions [#2489] ([#2497](https://github.com/rolter-ai/rolter/pull/2497)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* apply guardrails, the webhook and plugins to realtime sessions [#1880] ([#2491](https://github.com/rolter-ai/rolter/pull/2491)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* keep the public sk-rolter-dev key off production paths ([#2481](https://github.com/rolter-ai/rolter/pull/2481)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* keep datastore credentials out of startup logs ([#2460](https://github.com/rolter-ai/rolter/pull/2460)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* prune guardrail overrides that name a paused or removed rule ([#2401](https://github.com/rolter-ai/rolter/pull/2401)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* validate prompt template versions and prune invalid ones from the snapshot ([#2426](https://github.com/rolter-ai/rolter/pull/2426)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* enforce the egress policy at connect time for control-plane webhooks ([#2393](https://github.com/rolter-ai/rolter/pull/2393)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* refuse budget periods and caps the gateway does not enforce [#1902] ([#2388](https://github.com/rolter-ai/rolter/pull/2388)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* deliver alert transitions to their webhook channel [#1871, #1872] ([#1953](https://github.com/rolter-ai/rolter/pull/1953)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* run provider queues concurrently and report redis and aborts [#1815] ([#1865](https://github.com/rolter-ai/rolter/pull/1865)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* scope analytics and health reads to the caller's tenancy [#1820] ([#1842](https://github.com/rolter-ai/rolter/pull/1842)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* meter realtime sessions against budgets and rate limits [#1396] ([#1913](https://github.com/rolter-ai/rolter/pull/1913)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* report lost ui-event and mcp-log writes on the server ([#1766](https://github.com/rolter-ai/rolter/pull/1766)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* read provider credentials in either spelling on seed and export ([#1733](https://github.com/rolter-ai/rolter/pull/1733)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* make a missing node identity audible ([#1705](https://github.com/rolter-ai/rolter/pull/1705)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* give ProviderConfig a Default so a new field is not a workspace-wide break [#1150] ([#1169](https://github.com/rolter-ai/rolter/pull/1169)) by [@ormeilu](https://github.com/ormeilu)

### Build
- *(core)* fix dead_code warning under --no-default-features ([#1408](https://github.com/rolter-ai/rolter/pull/1408)) by [@ormeilu](https://github.com/ormeilu)

### Documentation
- *(core)* document the file-configurable [security] keys [#2551] ([#2634](https://github.com/rolter-ai/rolter/pull/2634)) by [@ormeilu](https://github.com/ormeilu)
- consolidate both doc trees under docs/ as user-docs and dev-docs [#1516] ([#1517](https://github.com/rolter-ai/rolter/pull/1517)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* correct the mcp stability notes after #952 ([#1449](https://github.com/rolter-ai/rolter/pull/1449)) by [@ormeilu](https://github.com/ormeilu)
- record what 1.0.0 guarantees on each api surface ([#1427](https://github.com/rolter-ai/rolter/pull/1427)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(gateway)* run experimental onnx local classifiers [#3443] ([#3584](https://github.com/rolter-ai/rolter/pull/3584)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* first-dialled variant and per-attempt spans [#3505] ([#3550](https://github.com/rolter-ai/rolter/pull/3550)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* mcp follow-ups, second round [#3466] ([#3533](https://github.com/rolter-ai/rolter/pull/3533)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* config export follow-ups [#3481] ([#3530](https://github.com/rolter-ai/rolter/pull/3530)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* local classifier follow-ups [#3452] ([#3518](https://github.com/rolter-ai/rolter/pull/3518)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* export one organization per file and carry the mcp marker [#3440] ([#3492](https://github.com/rolter-ai/rolter/pull/3492)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* per-org namespaces for routes, providers and provider groups [#1857] ([#3465](https://github.com/rolter-ai/rolter/pull/3465)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* run semantic guard providers as in-process classifiers [#3329] ([#3456](https://github.com/rolter-ai/rolter/pull/3456)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* mcp group assignments and tools/list filtering [#3295] ([#3439](https://github.com/rolter-ai/rolter/pull/3439)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* fallback failure classes and response guard follow-ups [#3319] ([#3437](https://github.com/rolter-ai/rolter/pull/3437)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* wave 4 control-plane ops follow-ups [#3311] ([#3417](https://github.com/rolter-ai/rolter/pull/3417)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* record request outcomes, admit one breaker probe [#3307] ([#3402](https://github.com/rolter-ai/rolter/pull/3402)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* let a fallback target declare which failure classes it takes over [#1584] ([#3376](https://github.com/rolter-ai/rolter/pull/3376)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* shared key rotation with overlap, org-scoped alerting, csv export, request-log sequence [#1837, #1829, #1838, #1392] ([#3364](https://github.com/rolter-ai/rolter/pull/3364)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* scoped semantic guard policies over several guardrail providers [#459] ([#3371](https://github.com/rolter-ai/rolter/pull/3371)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* negotiate a snapshot format version between control plane and gateway [#1512] ([#3372](https://github.com/rolter-ai/rolter/pull/3372)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* restore pii placeholders on streamed responses, capture the pre-restore body [#911, #3240] ([#3357](https://github.com/rolter-ai/rolter/pull/3357)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* external specification source registry, provenance in both openapi documents [#907] ([#3358](https://github.com/rolter-ai/rolter/pull/3358)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* mcp tool discovery, enforced tool groups and org settings, oauth scope step-up, connection status [#1283, #1403, #1404, #1417, #951] ([#3318](https://github.com/rolter-ai/rolter/pull/3318)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* cap spend and rate per key at mint and per person, carried by config export [#1171, #1830, #1312, #3128] ([#3312](https://github.com/rolter-ai/rolter/pull/3312)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* redact secrets in captured payloads and erase a subject's logs on request [#1835, #1085] ([#3310](https://github.com/rolter-ai/rolter/pull/3310)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* independent model catalogue, /v1/models metadata and whole ids, deprecation registry [#1729, #2939, #3245, #1423] ([#3288](https://github.com/rolter-ai/rolter/pull/3288)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add an infinity embeddings and reranking provider kind [#463] ([#3260](https://github.com/rolter-ai/rolter/pull/3260)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* support the system one decision api [#3150, #2535] ([#3156](https://github.com/rolter-ai/rolter/pull/3156)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* price realtime audio and text tokens separately and meter input transcription [#1882, #1883] ([#3142](https://github.com/rolter-ai/rolter/pull/3142)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* price image and audio requests by unit [#2937] ([#3120](https://github.com/rolter-ai/rolter/pull/3120)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* per-route request, token and concurrency limits [#2936] ([#3110](https://github.com/rolter-ai/rolter/pull/3110)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* dry-run the whole guardrail policy on a sample [#3084] ([#3105](https://github.com/rolter-ai/rolter/pull/3105)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* name guardrail providers in outcome events and log realtime plugin calls [#3038, #3071] ([#3086](https://github.com/rolter-ai/rolter/pull/3086)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* test a guardrail rule or provider on a sample before publishing [#2159, #2265] ([#3083](https://github.com/rolter-ai/rolter/pull/3083)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* serve the public base url to every signed-in caller [#2234, #2269] ([#3064](https://github.com/rolter-ai/rolter/pull/3064)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* structured omitted entries on config problems [#1876] ([#3042](https://github.com/rolter-ai/rolter/pull/3042)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* record guardrail outcomes per rule [#2158] ([#3034](https://github.com/rolter-ai/rolter/pull/3034)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* show spend so far against each budget on the limits screen [#2091] ([#3023](https://github.com/rolter-ai/rolter/pull/3023)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* store kv-events and lmcache settings on database providers [#2236] ([#2995](https://github.com/rolter-ai/rolter/pull/2995)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* price anthropic 1 hour cache writes at their own rate [#2891] ([#2901](https://github.com/rolter-ai/rolter/pull/2901)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* price tokens written to the prompt cache [#2876] ([#2892](https://github.com/rolter-ai/rolter/pull/2892)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* give common validation and conflict errors stable codes [#2567] ([#2727](https://github.com/rolter-ai/rolter/pull/2727)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* warn when a standalone gateway config sets project_scoped ([#2604](https://github.com/rolter-ai/rolter/pull/2604)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* let providers and groups be scoped to one project ([#2470](https://github.com/rolter-ai/rolter/pull/2470)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* record proxied mcp tool calls in the mcp tool-call log [#2395] ([#2431](https://github.com/rolter-ai/rolter/pull/2431)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* enrol a second factor at sign-in when the org requires one [#1852] ([#1906](https://github.com/rolter-ai/rolter/pull/1906)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* list a provider's probed catalogue in /v1/models [#1647] ([#1726](https://github.com/rolter-ai/rolter/pull/1726)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* translate the experimental note in the nav rail ([#1535](https://github.com/rolter-ai/rolter/pull/1535)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* migrate rolter.toml across schema versions on load [#326] ([#1515](https://github.com/rolter-ai/rolter/pull/1515)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* warn about unrecognised rolter.toml keys at startup ([#1438](https://github.com/rolter-ai/rolter/pull/1438)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* spend stored MCP credentials and per-server timeouts ([#1446](https://github.com/rolter-ai/rolter/pull/1446)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* report unrecognised rolter.toml keys in rolter check ([#1433](https://github.com/rolter-ai/rolter/pull/1433)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* stability marker so experimental subsystems are labelled [#1385] ([#1402](https://github.com/rolter-ai/rolter/pull/1402)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* per-budget override for unpriced_policy [#996] ([#1286](https://github.com/rolter-ai/rolter/pull/1286)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* require a name and an expiry when minting a virtual key [#945] ([#1172](https://github.com/rolter-ai/rolter/pull/1172)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* carry the security policy to the gateway and enforce it [#1162] ([#1165](https://github.com/rolter-ai/rolter/pull/1165)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* throttle and audit failed logins [#1079] ([#1161](https://github.com/rolter-ai/rolter/pull/1161)) by [@ormeilu](https://github.com/ormeilu)

### Miscellaneous
- *(gateway)* document websocket as ingest-only mcp transport ([#2714](https://github.com/rolter-ai/rolter/pull/2714)) by [@ormeilu](https://github.com/ormeilu)

### Refactoring
- *(core)* a Default for route Target and VirtualKeyRecord [#3332] ([#3531](https://github.com/rolter-ai/rolter/pull/3531)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* drop the unread additional_fields from AdvancedModelConfig ([#1715](https://github.com/rolter-ai/rolter/pull/1715)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* compute money in exact decimal instead of f64 ([#1450](https://github.com/rolter-ai/rolter/pull/1450)) by [@ormeilu](https://github.com/ormeilu)

### Testing
- generate the remaining literal test credentials at run time [#2685] ([#2718](https://github.com/rolter-ai/rolter/pull/2718)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.11](https://github.com/rolter-ai/rolter/compare/rolter-core-v0.0.10...rolter-core-v0.0.11) - 2026-08-13

### Bug Fixes
- *(ui)* make the base-URL hint kind-aware and preview the resolved path ([#981](https://github.com/rolter-ai/rolter/pull/981)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* drive the currency chooser from the configured rate table ([#978](https://github.com/rolter-ai/rolter/pull/978)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* serve a partial snapshot instead of withholding the fleet ([#973](https://github.com/rolter-ai/rolter/pull/973)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* drop the inert guardrail default_on flag ([#759](https://github.com/rolter-ai/rolter/pull/759)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* re-check the egress policy at connect time ([#662](https://github.com/rolter-ai/rolter/pull/662)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* make model price currency real and extensible ([#661](https://github.com/rolter-ai/rolter/pull/661)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* deny link-local egress so a provider api_base can't reach metadata ([#655](https://github.com/rolter-ai/rolter/pull/655)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* omit unservable rows from the snapshot instead of 500ing ([#654](https://github.com/rolter-ai/rolter/pull/654)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* fail closed when the virtual-key set is empty ([#653](https://github.com/rolter-ai/rolter/pull/653)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(gateway)* make unpriced traffic an explicit budget policy ([#997](https://github.com/rolter-ai/rolter/pull/997)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* test a provider's endpoint and credential from the dashboard ([#935](https://github.com/rolter-ai/rolter/pull/935)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* external PII sanitizer adapter with opt-in restoration ([#910](https://github.com/rolter-ai/rolter/pull/910)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* predicted-latency scheduling ([#912](https://github.com/rolter-ai/rolter/pull/912)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* instrument snapshot generation and the CRUD API [#845] ([#909](https://github.com/rolter-ai/rolter/pull/909)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add lora-aware scheduling strategy [#853] ([#896](https://github.com/rolter-ai/rolter/pull/896)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* plugin dispatch runtime [#509] ([#867](https://github.com/rolter-ai/rolter/pull/867)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* add ROLTER_TELEMETRY_ENABLED as an explicit telemetry off [#812] ([#827](https://github.com/rolter-ai/rolter/pull/827)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* conform to OTel GenAI semantic conventions [#808] ([#829](https://github.com/rolter-ai/rolter/pull/829)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* export logs, process and resource telemetry over OTLP [#809] ([#828](https://github.com/rolter-ai/rolter/pull/828)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway,core)* export labelled counters and histograms over OTLP [#805] ([#822](https://github.com/rolter-ai/rolter/pull/822)) by [@ormeilu](https://github.com/ormeilu)
- *(ui,control)* dashboard telemetry — browser tracing, runtime config, UX event ingest and emitters [#805] ([#811](https://github.com/rolter-ai/rolter/pull/811)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway,infra)* fix trace parenting, span the pipeline, add an observability overlay [#805] ([#806](https://github.com/rolter-ai/rolter/pull/806)) by [@ormeilu](https://github.com/ormeilu)
- *(ui,control)* build client and model settings screens [#564] ([#804](https://github.com/rolter-ai/rolter/pull/804)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* enforce access-profile model and route policy [#791] ([#803](https://github.com/rolter-ai/rolter/pull/803)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* enforce MCP OAuth authorization ([#773](https://github.com/rolter-ai/rolter/pull/773)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add adaptive routing strategy ([#701](https://github.com/rolter-ai/rolter/pull/701)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* manage cross-dialect compatibility policy ([#696](https://github.com/rolter-ai/rolter/pull/696)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* cap spend by business unit and customer ([#695](https://github.com/rolter-ai/rolter/pull/695)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* attribute virtual-key spend to business units ([#688](https://github.com/rolter-ai/rolter/pull/688)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* register Gemini Interactions provider kind by [@ormeilu](https://github.com/ormeilu)
- *(control)* persist global runtime policy settings by [@ormeilu](https://github.com/ormeilu)
- *(store)* merge published prompt templates into snapshots by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* enforce post_call guardrails on non-streaming responses ([#667](https://github.com/rolter-ai/rolter/pull/667)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* per-route guardrail enable/disable overrides ([#664](https://github.com/rolter-ai/rolter/pull/664)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* expand provider adapter kind coverage [ROL-132] ([#645](https://github.com/rolter-ai/rolter/pull/645)) by [@ormeilu](https://github.com/ormeilu)

### Miscellaneous
- *(control)* align MCP transport allowlists across control plane and schema ([#794](https://github.com/rolter-ai/rolter/pull/794)) by [@ormeilu](https://github.com/ormeilu)

### Performance
- *(core)* optimize slugify to reduce heap allocations [ROL-PERF] ([#844](https://github.com/rolter-ai/rolter/pull/844)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* eliminate intermediate string join allocations [ROL-PERF] ([#862](https://github.com/rolter-ai/rolter/pull/862)) by [@ormeilu](https://github.com/ormeilu)

### Refactoring
- *(core)* audit telemetry helpers against OTel don't-wrap guidance ([#823](https://github.com/rolter-ai/rolter/pull/823)) by [@ormeilu](https://github.com/ormeilu)

### Testing
- *(core)* cover unicode lowercase expansion in slugify [ROL-PERF] ([#864](https://github.com/rolter-ai/rolter/pull/864)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* add unit tests for normalize_code ([#717](https://github.com/rolter-ai/rolter/pull/717)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.10](https://github.com/rolter-ai/rolter/compare/rolter-core-v0.0.9...rolter-core-v0.0.10) - 2026-07-21

### Features
- *(proxy)* add xai (grok) hosted provider kind ([#600](https://github.com/rolter-ai/rolter/pull/600)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add gemini/mistral/groq + native gemini generateContent kinds ([#598](https://github.com/rolter-ai/rolter/pull/598)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add versioned prompt templates and route decorators ([#594](https://github.com/rolter-ai/rolter/pull/594)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add custom guardrail webhook ([#593](https://github.com/rolter-ai/rolter/pull/593)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add built-in regex guardrails and PII redactor ([#592](https://github.com/rolter-ai/rolter/pull/592)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* uniform readonly/default tier wrapper for providers and groups ([#579](https://github.com/rolter-ai/rolter/pull/579)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* resolve group-slug/model provider group addressing ([#572](https://github.com/rolter-ai/rolter/pull/572)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* persist advanced model config by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* implement medium-priority platform enhancements [ROL-65] ([#525](https://github.com/rolter-ai/rolter/pull/525)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add precise and LMCache-aware routing [ROL-54] ([#522](https://github.com/rolter-ai/rolter/pull/522)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* add semantic response cache [ROL-57] ([#521](https://github.com/rolter-ai/rolter/pull/521)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add rotating egress proxy pools [ROL-101] ([#520](https://github.com/rolter-ai/rolter/pull/520)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.9](https://github.com/rolter-ai/rolter/compare/rolter-core-v0.0.8...rolter-core-v0.0.9) - 2026-07-15

### Features
- *(store)* add immutable URL-safe provider slug for model addressing ([#191](https://github.com/rolter-ai/rolter/pull/191)) by [@ormeilu](https://github.com/ormeilu)
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
