# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [1.0.0](https://github.com/rolter-ai/rolter/compare/rolter-store-v0.1.0...rolter-store-v1.0.0) - 2026-11-01

### Bug Fixes
- *(gateway)* harden local classifier loading and scoring [#3511] ([#3574](https://github.com/rolter-ai/rolter/pull/3574)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* narrowed org rows fall through to global [#3546] ([#3572](https://github.com/rolter-ai/rolter/pull/3572)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* key_hash out of responses, org model labels [#3525] ([#3560](https://github.com/rolter-ai/rolter/pull/3560)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* keep an org row a bootstrap row shares a name with [#3516] ([#3552](https://github.com/rolter-ai/rolter/pull/3552)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* forward only an mcp body the relay has judged [#3527] ([#3551](https://github.com/rolter-ai/rolter/pull/3551)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* analytics follow-ups, second round [#3486] ([#3534](https://github.com/rolter-ai/rolter/pull/3534)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* routing checks and provider profiles [#3430] ([#3503](https://github.com/rolter-ai/rolter/pull/3503)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* gateway request path follow-ups [#3404] ([#3491](https://github.com/rolter-ai/rolter/pull/3491)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* per-org namespace follow-ups [#3459] ([#3490](https://github.com/rolter-ai/rolter/pull/3490)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* control-plane operations follow-ups [#3433] ([#3489](https://github.com/rolter-ai/rolter/pull/3489)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* snapshot format follow-ups [#3347] ([#3438](https://github.com/rolter-ai/rolter/pull/3438)) by [@ormeilu](https://github.com/ormeilu)
- close ten more post-1.0 polish issues [#1133, #3147, #1916] ([#3215](https://github.com/rolter-ai/rolter/pull/3215)) by [@ormeilu](https://github.com/ormeilu)
- close ten post-1.0 polish issues [#1915, #1987, #3171, #3148] ([#3209](https://github.com/rolter-ai/rolter/pull/3209)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* make the audit log's end_at exclusive as documented [#3141] ([#3207](https://github.com/rolter-ai/rolter/pull/3207)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* [**breaking**] deliver alerts outside the lock, bound api writes, refuse short admin tokens, warn on a public dev key [#1967, #1925, #1964, #1965] ([#3146](https://github.com/rolter-ai/rolter/pull/3146)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* scope config problems, budgets and rate limits to the caller's organizations [#3100] ([#3122](https://github.com/rolter-ai/rolter/pull/3122)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* keep the snapshot serving when a provider is left out [#2950, #2946, #2947] ([#3041](https://github.com/rolter-ai/rolter/pull/3041)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* load a route's targets into the snapshot in a stable order [#2962] ([#3006](https://github.com/rolter-ai/rolter/pull/3006)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* fail closed when security or egress settings do not parse [#2943] ([#2948](https://github.com/rolter-ai/rolter/pull/2948)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* fail closed when a route's advanced settings do not parse [#2938] ([#2944](https://github.com/rolter-ai/rolter/pull/2944)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* serve dashboard-owned tables from the store in merged mode [#2922] ([#2941](https://github.com/rolter-ai/rolter/pull/2941)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* reject negative and NaN price rates, retire the unread route cache-write field [#2889, #2898, #2890] ([#2900](https://github.com/rolter-ai/rolter/pull/2900)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* superadmin mcp consent, lifecycle operation and account reads [#2859] ([#2874](https://github.com/rolter-ai/rolter/pull/2874)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* record the signed-in superadmin as the audit actor [#2844, #2857] ([#2862](https://github.com/rolter-ai/rolter/pull/2862)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* guard an org's last admin on scim and sso group sync ([#2671](https://github.com/rolter-ai/rolter/pull/2671)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* refuse deleting a referenced provider with a 409 ([#2600](https://github.com/rolter-ai/rolter/pull/2600)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* hide and refuse provider sign-in while single sign-on is off ([#2607](https://github.com/rolter-ai/rolter/pull/2607)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* refuse revoking an org's last admin grant ([#2556](https://github.com/rolter-ai/rolter/pull/2556)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* remove the virtual-key switch that never changed a decision ([#2550](https://github.com/rolter-ai/rolter/pull/2550)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* record the provider's org in health events ([#2493](https://github.com/rolter-ai/rolter/pull/2493)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* refuse to demote, deactivate or delete the last active superadmin [#2344] ([#2472](https://github.com/rolter-ai/rolter/pull/2472)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* replace a pending invitation instead of answering 500 ([#2473](https://github.com/rolter-ai/rolter/pull/2473)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* remove the unenforced dashboard password setting ([#2458](https://github.com/rolter-ai/rolter/pull/2458)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* refuse turning off the last sso provider while password sign-in is off ([#2442](https://github.com/rolter-ai/rolter/pull/2442)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* end a browser sso sign-in on the dashboard with a one-time code ([#2410](https://github.com/rolter-ai/rolter/pull/2410)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* let alert rules fire on no data and below a threshold [#2232] ([#2422](https://github.com/rolter-ai/rolter/pull/2422)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* refuse budget periods and caps the gateway does not enforce [#1902] ([#2388](https://github.com/rolter-ai/rolter/pull/2388)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* revoke live sessions on a superadmin password reset ([#1962](https://github.com/rolter-ai/rolter/pull/1962)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* scope analytics and health reads to the caller's tenancy [#1820] ([#1842](https://github.com/rolter-ai/rolter/pull/1842)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* make the ui_events opt-out reachable in a postgres deployment [#1748] ([#1767](https://github.com/rolter-ai/rolter/pull/1767)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* bump config_version on provider group writes ([#1702](https://github.com/rolter-ai/rolter/pull/1702)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* only demand a kek when set_oauth_client seals a secret [#1564] ([#1571](https://github.com/rolter-ai/rolter/pull/1571)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* validate set_auth before sealing, and only demand a kek to seal ([#1565](https://github.com/rolter-ai/rolter/pull/1565)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* clear the mcp oauth discovery cache when the issuer or discovery mode changes [#1432] ([#1507](https://github.com/rolter-ai/rolter/pull/1507)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* clear stale mcp oauth discovery cache on url change ([#1431](https://github.com/rolter-ai/rolter/pull/1431)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* bring the mcp oauth client up to the current spec ([#1419](https://github.com/rolter-ai/rolter/pull/1419)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* give ProviderConfig a Default so a new field is not a workspace-wide break [#1150] ([#1169](https://github.com/rolter-ai/rolter/pull/1169)) by [@ormeilu](https://github.com/ormeilu)

### Build
- *(store)* reach rolter-store optional deps only through dep: names [#1442] ([#2918](https://github.com/rolter-ai/rolter/pull/2918)) by [@ormeilu](https://github.com/ormeilu)

### CI/CD
- isolate the sso e2e journeys and run the examples tests [#3368] ([#3474](https://github.com/rolter-ai/rolter/pull/3474)) by [@ormeilu](https://github.com/ormeilu)
- close nine ci and test reliability issues [#2771, #2770, #2736] ([#3229](https://github.com/rolter-ai/rolter/pull/3229)) by [@ormeilu](https://github.com/ormeilu)

### Dependencies
- *(deps)* bump rust dependencies to their latest releases [#2906] ([#2914](https://github.com/rolter-ai/rolter/pull/2914)) by [@ormeilu](https://github.com/ormeilu)

### Documentation
- consolidate both doc trees under docs/ as user-docs and dev-docs [#1516] ([#1517](https://github.com/rolter-ai/rolter/pull/1517)) by [@ormeilu](https://github.com/ormeilu)
- record what 1.0.0 guarantees on each api surface ([#1427](https://github.com/rolter-ai/rolter/pull/1427)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(control)* persist local semantic truncation settings [#3558, #3568] ([#3599](https://github.com/rolter-ai/rolter/pull/3599)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* download csv exports through single-use tickets [#3504] ([#3549](https://github.com/rolter-ai/rolter/pull/3549)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* mcp follow-ups, second round [#3466] ([#3533](https://github.com/rolter-ai/rolter/pull/3533)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* config export follow-ups [#3481] ([#3530](https://github.com/rolter-ai/rolter/pull/3530)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* local classifier follow-ups [#3452] ([#3518](https://github.com/rolter-ai/rolter/pull/3518)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* export one organization per file and carry the mcp marker [#3440] ([#3492](https://github.com/rolter-ai/rolter/pull/3492)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* mcp follow-ups: logging, recorded scopes and key groups [#3399] ([#3479](https://github.com/rolter-ai/rolter/pull/3479)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* per-org namespaces for routes, providers and provider groups [#1857] ([#3465](https://github.com/rolter-ai/rolter/pull/3465)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* run semantic guard providers as in-process classifiers [#3329] ([#3456](https://github.com/rolter-ai/rolter/pull/3456)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* mcp group assignments and tools/list filtering [#3295] ([#3439](https://github.com/rolter-ai/rolter/pull/3439)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* fallback failure classes and response guard follow-ups [#3319] ([#3437](https://github.com/rolter-ai/rolter/pull/3437)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* wave 4 control-plane ops follow-ups [#3311] ([#3417](https://github.com/rolter-ai/rolter/pull/3417)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* let a fallback target declare which failure classes it takes over [#1584] ([#3376](https://github.com/rolter-ai/rolter/pull/3376)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* shared key rotation with overlap, org-scoped alerting, csv export, request-log sequence [#1837, #1829, #1838, #1392] ([#3364](https://github.com/rolter-ai/rolter/pull/3364)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* scoped semantic guard policies over several guardrail providers [#459] ([#3371](https://github.com/rolter-ai/rolter/pull/3371)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* database-backed route variants and the dashboard variant editor [#391] ([#3363](https://github.com/rolter-ai/rolter/pull/3363)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* provider cache-metrics labels, more cache-hit readers, price catalogue import [#2882, #984, #966] ([#3354](https://github.com/rolter-ai/rolter/pull/3354)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* operator and security auditor deployment roles, scoped control-plane api tokens [#1834, #1836] ([#3327](https://github.com/rolter-ai/rolter/pull/3327)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* mcp tool discovery, enforced tool groups and org settings, oauth scope step-up, connection status [#1283, #1403, #1404, #1417, #951] ([#3318](https://github.com/rolter-ai/rolter/pull/3318)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* ldap sign-in, per-org mfa enrolment, admin-token sign-in and emailed invitations [#1826, #1909, #1828, #1590] ([#3313](https://github.com/rolter-ai/rolter/pull/3313)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* fleet breaker state and summary, scoped provider health, batch reads, withheld-usage reporting [#2254, #2013, #1833, #1996, #1774, #1395] ([#3315](https://github.com/rolter-ai/rolter/pull/3315)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* cap spend and rate per key at mint and per person, carried by config export [#1171, #1830, #1312, #3128] ([#3312](https://github.com/rolter-ai/rolter/pull/3312)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* redact secrets in captured payloads and erase a subject's logs on request [#1835, #1085] ([#3310](https://github.com/rolter-ai/rolter/pull/3310)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* add an infinity embeddings and reranking provider kind [#463] ([#3260](https://github.com/rolter-ai/rolter/pull/3260)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* support the system one decision api [#3150, #2535] ([#3156](https://github.com/rolter-ai/rolter/pull/3156)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* rename a team, filter users by team, sentence-case sheet hints [#3126, #3127, #3123] ([#3143](https://github.com/rolter-ai/rolter/pull/3143)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* price realtime audio and text tokens separately and meter input transcription [#1882, #1883] ([#3142](https://github.com/rolter-ai/rolter/pull/3142)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* stream the filtered audit log as csv or json [#3051] ([#3135](https://github.com/rolter-ai/rolter/pull/3135)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* list budgets and rate limits across scopes, cost attribution polish [#2093, #2111] ([#3134](https://github.com/rolter-ai/rolter/pull/3134)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* price image and audio requests by unit [#2937] ([#3120](https://github.com/rolter-ai/rolter/pull/3120)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* per-route request, token and concurrency limits [#2936] ([#3110](https://github.com/rolter-ai/rolter/pull/3110)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* list profile assignments by subject and code the remaining crud 400s [#3025, #3069] ([#3108](https://github.com/rolter-ai/rolter/pull/3108)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* dry-run the whole guardrail policy on a sample [#3084] ([#3105](https://github.com/rolter-ai/rolter/pull/3105)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* change a member's role, name invite senders, reset a password [#2310, #2325, #2240] ([#3080](https://github.com/rolter-ai/rolter/pull/3080)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* name guardrail providers in outcome events and log realtime plugin calls [#3038, #3071] ([#3086](https://github.com/rolter-ai/rolter/pull/3086)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* code the remaining validation refusals and name the invalid field [#2729, #2735] ([#3062](https://github.com/rolter-ai/rolter/pull/3062)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* page alert history, code delivery errors and record readings [#2333, #2334, #2301] ([#3045](https://github.com/rolter-ai/rolter/pull/3045)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* store kv-events and lmcache settings on database providers [#2236] ([#2995](https://github.com/rolter-ai/rolter/pull/2995)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* edit a route's strategy, targets and weights [#2235] ([#2973](https://github.com/rolter-ai/rolter/pull/2973)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* price anthropic 1 hour cache writes at their own rate [#2891] ([#2901](https://github.com/rolter-ai/rolter/pull/2901)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* price tokens written to the prompt cache [#2876] ([#2892](https://github.com/rolter-ai/rolter/pull/2892)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* let a signed-in account change its own password [#2804] ([#2833](https://github.com/rolter-ai/rolter/pull/2833)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* sync display names from scim patch and default them from oidc [#2435] ([#2730](https://github.com/rolter-ai/rolter/pull/2730)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* report whether a provider has a stored key on its DTO [#2437] ([#2709](https://github.com/rolter-ai/rolter/pull/2709)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* give common validation and conflict errors stable codes [#2567] ([#2727](https://github.com/rolter-ai/rolter/pull/2727)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* count live virtual keys on business unit and customer listings ([#2675](https://github.com/rolter-ai/rolter/pull/2675)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* edit an alert rule's comparison and no-data policy [#2423] ([#2504](https://github.com/rolter-ai/rolter/pull/2504)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* let providers and groups be scoped to one project ([#2470](https://github.com/rolter-ai/rolter/pull/2470)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* let every account save private filter presets [#1825] ([#2451](https://github.com/rolter-ai/rolter/pull/2451)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* persist a user's preferences and defaults server-side [#1824] ([#2447](https://github.com/rolter-ai/rolter/pull/2447)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* let every account edit its own display name and bio [#1823] ([#2433](https://github.com/rolter-ai/rolter/pull/2433)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add a deployment-wide audit log read for superadmins ([#2399](https://github.com/rolter-ai/rolter/pull/2399)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* enrol a second factor at sign-in when the org requires one [#1852] ([#1906](https://github.com/rolter-ai/rolter/pull/1906)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* edit budgets and rate limits in place [#1285] ([#1907](https://github.com/rolter-ai/rolter/pull/1907)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* mint a playground key the server scopes itself [#1640] ([#1706](https://github.com/rolter-ai/rolter/pull/1706)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* list every project in an org in one request ([#1443](https://github.com/rolter-ai/rolter/pull/1443)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* spend stored MCP credentials and per-server timeouts ([#1446](https://github.com/rolter-ai/rolter/pull/1446)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* static credentials and per-server transport settings for mcp [#952] ([#1349](https://github.com/rolter-ai/rolter/pull/1349)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* label primitive for providers, routes, groups and models [#985] ([#1330](https://github.com/rolter-ai/rolter/pull/1330)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* totp second factor for local accounts [#1078] ([#1324](https://github.com/rolter-ai/rolter/pull/1324)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* update an SSO provider in place instead of delete-and-recreate [#1233] ([#1299](https://github.com/rolter-ai/rolter/pull/1299)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* per-budget override for unpriced_policy [#996] ([#1286](https://github.com/rolter-ai/rolter/pull/1286)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* say whether an SSO provider has a client secret stored ([#1278](https://github.com/rolter-ai/rolter/pull/1278)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* verify and rotate the KEK against the control-plane store [#923] ([#1175](https://github.com/rolter-ai/rolter/pull/1175)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* require a name and an expiry when minting a virtual key [#945] ([#1172](https://github.com/rolter-ai/rolter/pull/1172)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* carry the security policy to the gateway and enforce it [#1162] ([#1165](https://github.com/rolter-ai/rolter/pull/1165)) by [@ormeilu](https://github.com/ormeilu)

### Miscellaneous
- *(store)* sweep every use-swept table on the periodic expiry pass [#2616] ([#3078](https://github.com/rolter-ai/rolter/pull/3078)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* expire idle sso state rows and list the sso tables in the data model [#2414] ([#2617](https://github.com/rolter-ai/rolter/pull/2617)) by [@ormeilu](https://github.com/ormeilu)

### Performance
- *(control)* serve model, route and complexity targets in one read per screen [#2210, #3031, #3046] ([#3091](https://github.com/rolter-ai/rolter/pull/3091)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* encode mfa hex with a lookup table instead of format! ([#1436](https://github.com/rolter-ai/rolter/pull/1436)) by [@ormeilu](https://github.com/ormeilu)

### Refactoring
- *(core)* a Default for route Target and VirtualKeyRecord [#3332] ([#3531](https://github.com/rolter-ai/rolter/pull/3531)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* split postgres repo into per-domain modules and drop its argument-list allows [#1042] ([#3256](https://github.com/rolter-ai/rolter/pull/3256)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* compute money in exact decimal instead of f64 ([#1450](https://github.com/rolter-ai/rolter/pull/1450)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* extract the guardrail repo into its own module [#1042] ([#1320](https://github.com/rolter-ai/rolter/pull/1320)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* extract mcp repo into its own module [#1042] ([#1303](https://github.com/rolter-ai/rolter/pull/1303)) by [@ormeilu](https://github.com/ormeilu)

### Testing
- generate the remaining literal test credentials at run time [#2685] ([#2718](https://github.com/rolter-ai/rolter/pull/2718)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* wait out the sweep lock instead of a fixed retry budget ([#2688](https://github.com/rolter-ai/rolter/pull/2688)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* never fall back to the shared test database ([#2690](https://github.com/rolter-ai/rolter/pull/2690)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* one shared test postgres per machine, smaller test pools [#1735, #1736] ([#1914](https://github.com/rolter-ai/rolter/pull/1914)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* give every worktree its own postgres test database [#1430] ([#1734](https://github.com/rolter-ai/rolter/pull/1734)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* drop the per-test schema when an integration test finishes ([#1428](https://github.com/rolter-ai/rolter/pull/1428)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.11](https://github.com/rolter-ai/rolter/compare/rolter-store-v0.0.10...rolter-store-v0.0.11) - 2026-08-13

### Bug Fixes
- *(store)* parse every balancing strategy the column can hold ([#931](https://github.com/rolter-ai/rolter/pull/931)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* restore migration 0018 checksum ([#724](https://github.com/rolter-ai/rolter/pull/724)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* make model price currency real and extensible ([#661](https://github.com/rolter-ai/rolter/pull/661)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* carry db-only snapshot fields through MergedConfigStore ([#652](https://github.com/rolter-ai/rolter/pull/652)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* decode budgets.limit_usd as text in snapshot load ([#628](https://github.com/rolter-ai/rolter/pull/628)) by [@ormeilu](https://github.com/ormeilu)

### CI/CD
- *(ci)* strengthen prek quality and security hooks ([#710](https://github.com/rolter-ai/rolter/pull/710)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(balancer)* predicted-latency scheduling ([#912](https://github.com/rolter-ai/rolter/pull/912)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add lora-aware scheduling strategy [#853] ([#896](https://github.com/rolter-ai/rolter/pull/896)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* plugin dispatch runtime [#509] ([#867](https://github.com/rolter-ai/rolter/pull/867)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* back the observability connectors screen with a real API [#511] ([#831](https://github.com/rolter-ai/rolter/pull/831)) by [@ormeilu](https://github.com/ormeilu)
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
- *(control)* persist and govern the adaptive routing policy ([#702](https://github.com/rolter-ai/rolter/pull/702)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add adaptive routing strategy ([#701](https://github.com/rolter-ai/rolter/pull/701)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* drain nodes out of service safely ([#700](https://github.com/rolter-ai/rolter/pull/700)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add cluster node inventory ([#699](https://github.com/rolter-ai/rolter/pull/699)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* manage request-log retention ([#698](https://github.com/rolter-ai/rolter/pull/698)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* manage cross-dialect compatibility policy ([#696](https://github.com/rolter-ai/rolter/pull/696)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* cap spend by business unit and customer ([#695](https://github.com/rolter-ai/rolter/pull/695)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* attribute virtual-key spend to business units ([#688](https://github.com/rolter-ai/rolter/pull/688)) by [@ormeilu](https://github.com/ormeilu)
- *(proxy)* register Gemini Interactions provider kind by [@ormeilu](https://github.com/ormeilu)
- *(control)* persist global runtime policy settings by [@ormeilu](https://github.com/ormeilu)
- *(store)* bump snapshot version on skills writes by [@ormeilu](https://github.com/ormeilu)
- *(store)* add skills repository persistence foundation by [@ormeilu](https://github.com/ormeilu)
- *(store)* bump snapshot version on prompt template writes by [@ormeilu](https://github.com/ormeilu)
- *(store)* merge published prompt templates into snapshots by [@ormeilu](https://github.com/ormeilu)
- *(store)* add prompt template persistence foundation by [@ormeilu](https://github.com/ormeilu)
- *(control)* add business unit and customer CRUD foundation by [@ormeilu](https://github.com/ormeilu)
- *(core)* expand provider adapter kind coverage [ROL-132] ([#645](https://github.com/rolter-ai/rolter/pull/645)) by [@ormeilu](https://github.com/ormeilu)

### Miscellaneous
- *(control)* align MCP transport allowlists across control plane and schema ([#794](https://github.com/rolter-ai/rolter/pull/794)) by [@ormeilu](https://github.com/ormeilu)
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
