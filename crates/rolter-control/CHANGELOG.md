# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
## [0.1.1](https://github.com/rolter-ai/rolter/compare/rolter-control-v0.1.0...rolter-control-v0.1.1) - 2026-10-01

### Bug Fixes
- *(control)* let a project viewer read the caps and role matrix that govern their keys ([#2530](https://github.com/rolter-ai/rolter/pull/2530)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* make provider and provider-group capabilities project-aware ([#2521](https://github.com/rolter-ai/rolter/pull/2521)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* require a session on the /gw gateway proxy [#2463] ([#2487](https://github.com/rolter-ai/rolter/pull/2487)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* apply the egress policy to every operator-written url [#2383] ([#2492](https://github.com/rolter-ai/rolter/pull/2492)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* key captured bodies on a gateway-minted log id [#1937] ([#2494](https://github.com/rolter-ai/rolter/pull/2494)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* record the provider's org in health events ([#2493](https://github.com/rolter-ai/rolter/pull/2493)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* send the provider probe through the connect-time egress client ([#2483](https://github.com/rolter-ai/rolter/pull/2483)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* refuse to demote, deactivate or delete the last active superadmin [#2344] ([#2472](https://github.com/rolter-ai/rolter/pull/2472)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* keep the public sk-rolter-dev key off production paths ([#2481](https://github.com/rolter-ai/rolter/pull/2481)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* replace a pending invitation instead of answering 500 ([#2473](https://github.com/rolter-ai/rolter/pull/2473)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* bound the analytics clickhouse client's connect and request time [#1951] ([#2474](https://github.com/rolter-ai/rolter/pull/2474)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* drop a connector's stored secret when its endpoint moves origin [#2403] ([#2479](https://github.com/rolter-ai/rolter/pull/2479)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* require a session for the config view and guard the open router [#1840] ([#2465](https://github.com/rolter-ai/rolter/pull/2465)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* stop telling operators the gateway needs ROLTER_KEK ([#2466](https://github.com/rolter-ai/rolter/pull/2466)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* keep datastore credentials out of startup logs ([#2460](https://github.com/rolter-ai/rolter/pull/2460)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* remove the unenforced dashboard password setting ([#2458](https://github.com/rolter-ai/rolter/pull/2458)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* never sign in to an existing account from an invitation ([#2274](https://github.com/rolter-ai/rolter/pull/2274)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* prune guardrail overrides that name a paused or removed rule ([#2401](https://github.com/rolter-ai/rolter/pull/2401)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* refuse turning off the last sso provider while password sign-in is off ([#2442](https://github.com/rolter-ai/rolter/pull/2442)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* validate prompt template versions and prune invalid ones from the snapshot ([#2426](https://github.com/rolter-ai/rolter/pull/2426)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* end a browser sso sign-in on the dashboard with a one-time code ([#2410](https://github.com/rolter-ai/rolter/pull/2410)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* let alert rules fire on no data and below a threshold [#2232] ([#2422](https://github.com/rolter-ai/rolter/pull/2422)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* drop static mcp servers from the open config view ([#2368](https://github.com/rolter-ai/rolter/pull/2368)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* answer rbac/effective at each capability's own scope ([#2375](https://github.com/rolter-ai/rolter/pull/2375)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* keep raw store errors out of 500 response bodies ([#2369](https://github.com/rolter-ai/rolter/pull/2369)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* enforce the egress policy at connect time for control-plane webhooks ([#2393](https://github.com/rolter-ai/rolter/pull/2393)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* exit easy-up on ctrl-c and sigterm and drain the control plane ([#2380](https://github.com/rolter-ai/rolter/pull/2380)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* polish llm logs unpriced, readiness and absent bodies ([#2450](https://github.com/rolter-ai/rolter/pull/2450)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* refuse budget periods and caps the gateway does not enforce [#1902] ([#2388](https://github.com/rolter-ai/rolter/pull/2388)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* refuse an sso slug outside the charset with a 400 ([#2379](https://github.com/rolter-ai/rolter/pull/2379)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* end mcp oauth consent on auth sessions, not raw json [#2166] ([#2299](https://github.com/rolter-ai/rolter/pull/2299)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* deliver alert transitions to their webhook channel [#1871, #1872] ([#1953](https://github.com/rolter-ai/rolter/pull/1953)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* revoke live sessions on a superadmin password reset ([#1962](https://github.com/rolter-ai/rolter/pull/1962)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* balance cache-aware routes and expose provider queues [#1851] ([#1863](https://github.com/rolter-ai/rolter/pull/1863)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* scope analytics and health reads to the caller's tenancy [#1820] ([#1842](https://github.com/rolter-ai/rolter/pull/1842)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* redact internal error details in auth 500 responses ([#1956](https://github.com/rolter-ai/rolter/pull/1956)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* refuse malformed since/until and cursor timestamps [#1192] ([#1917](https://github.com/rolter-ai/rolter/pull/1917)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* redact store errors in scim 500 responses ([#1868](https://github.com/rolter-ai/rolter/pull/1868)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* redact database error details in collector config endpoint ([#1795](https://github.com/rolter-ai/rolter/pull/1795)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* redact internal store errors from snapshot 500 responses ([#1808](https://github.com/rolter-ai/rolter/pull/1808)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* make the ui_events opt-out reachable in a postgres deployment [#1748] ([#1767](https://github.com/rolter-ai/rolter/pull/1767)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* report lost ui-event and mcp-log writes on the server ([#1766](https://github.com/rolter-ai/rolter/pull/1766)) by [@ormeilu](https://github.com/ormeilu)
- *(ci)* repin rust-toolchain to its moved v1 and unrot a dated test ([#1762](https://github.com/rolter-ai/rolter/pull/1762)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* read provider credentials in either spelling on seed and export ([#1733](https://github.com/rolter-ai/rolter/pull/1733)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* read a route complexity policy at route:read ([#1717](https://github.com/rolter-ai/rolter/pull/1717)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* expose discovered_iss_supported on the oauth-client view [#1569] ([#1602](https://github.com/rolter-ai/rolter/pull/1602)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* make a missing node identity audible ([#1705](https://github.com/rolter-ai/rolter/pull/1705)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* bump config_version on provider group writes ([#1702](https://github.com/rolter-ai/rolter/pull/1702)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* record when a ui event happened, not when it arrived [#1224] ([#1668](https://github.com/rolter-ai/rolter/pull/1668)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* only demand a kek when set_oauth_client seals a secret [#1564] ([#1571](https://github.com/rolter-ai/rolter/pull/1571)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* validate set_auth before sealing, and only demand a kek to seal ([#1565](https://github.com/rolter-ai/rolter/pull/1565)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* clear the mcp oauth discovery cache when the issuer or discovery mode changes [#1432] ([#1507](https://github.com/rolter-ai/rolter/pull/1507)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* clear stale mcp oauth discovery cache on url change ([#1431](https://github.com/rolter-ai/rolter/pull/1431)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* bring the mcp oauth client up to the current spec ([#1419](https://github.com/rolter-ai/rolter/pull/1419)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* break invocation-list ties so burst rows page stably ([#1391](https://github.com/rolter-ai/rolter/pull/1391)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* document config/export and sso-provider update in the OpenAPI doc ([#1316](https://github.com/rolter-ai/rolter/pull/1316)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* return the unpriced flag on /analytics/invocations [#1226] ([#1277](https://github.com/rolter-ai/rolter/pull/1277)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* name the grain of a health rollup and nest targets under their provider ([#1282](https://github.com/rolter-ai/rolter/pull/1282)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* stop echoing raw ClickHouse errors to the dashboard [#1221] ([#1222](https://github.com/rolter-ai/rolter/pull/1222)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* stamp request logs with the request time, not the batch flush time ([#1223](https://github.com/rolter-ai/rolter/pull/1223)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* redact every credential from the dashboard's config view [#1212] ([#1216](https://github.com/rolter-ai/rolter/pull/1216)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* tell a store-less control plane apart from a generic 404 [#1204] ([#1211](https://github.com/rolter-ai/rolter/pull/1211)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* parse optional log-window params without failing on empty strings [#1177] ([#1190](https://github.com/rolter-ai/rolter/pull/1190)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* give the fleet one place to decide where request logs go [#929] ([#1174](https://github.com/rolter-ai/rolter/pull/1174)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* give ProviderConfig a Default so a new field is not a workspace-wide break [#1150] ([#1169](https://github.com/rolter-ai/rolter/pull/1169)) by [@ormeilu](https://github.com/ormeilu)

### Build
- *(control)* fix easy_up Args literal for rolter-control/postgres [#1295] ([#1305](https://github.com/rolter-ai/rolter/pull/1305)) by [@ormeilu](https://github.com/ormeilu)

### CI/CD
- *(infra)* repin rust-toolchain to a reachable commit [#1227] ([#1455](https://github.com/rolter-ai/rolter/pull/1455)) by [@ormeilu](https://github.com/ormeilu)

### Dependencies
- *(deps)* bump argon2 from 0.5.3 to 0.6.0 ([#1156](https://github.com/rolter-ai/rolter/pull/1156)) by [@dependabot[bot]](https://github.com/dependabot[bot])

### Documentation
- *(control)* document every route's query string, and guard it [#1412] ([#1604](https://github.com/rolter-ai/rolter/pull/1604)) by [@ormeilu](https://github.com/ormeilu)
- consolidate both doc trees under docs/ as user-docs and dev-docs [#1516] ([#1517](https://github.com/rolter-ai/rolter/pull/1517)) by [@ormeilu](https://github.com/ormeilu)
- record what 1.0.0 guarantees on each api surface ([#1427](https://github.com/rolter-ai/rolter/pull/1427)) by [@ormeilu](https://github.com/ormeilu)

### Features
- *(control)* expose gateway_base_url on /auth/me for every role [#2512] ([#2523](https://github.com/rolter-ai/rolter/pull/2523)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* let providers and groups be scoped to one project ([#2470](https://github.com/rolter-ai/rolter/pull/2470)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* let every account save private filter presets [#1825] ([#2451](https://github.com/rolter-ai/rolter/pull/2451)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* persist a user's preferences and defaults server-side [#1824] ([#2447](https://github.com/rolter-ai/rolter/pull/2447)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* record proxied mcp tool calls in the mcp tool-call log [#2395] ([#2431](https://github.com/rolter-ai/rolter/pull/2431)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* let every account edit its own display name and bio [#1823] ([#2433](https://github.com/rolter-ai/rolter/pull/2433)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* add a deployment-wide audit log read for superadmins ([#2399](https://github.com/rolter-ai/rolter/pull/2399)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* let members read the mcp logs of their own projects [#1831] ([#2397](https://github.com/rolter-ai/rolter/pull/2397)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* show the sso redirect uri to register in the identity provider ([#2308](https://github.com/rolter-ai/rolter/pull/2308)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* enrol a second factor at sign-in when the org requires one [#1852] ([#1906](https://github.com/rolter-ai/rolter/pull/1906)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* edit budgets and rate limits in place [#1285] ([#1907](https://github.com/rolter-ai/rolter/pull/1907)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* record the struggle signals the ux stream cannot express [#1731] ([#1746](https://github.com/rolter-ai/rolter/pull/1746)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* mint a playground key the server scopes itself [#1640] ([#1706](https://github.com/rolter-ai/rolter/pull/1706)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* give the dashboard a documentation base URL ([#1656](https://github.com/rolter-ai/rolter/pull/1656)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* export the tiered provider shape with a schema_version stamp ([#1561](https://github.com/rolter-ai/rolter/pull/1561)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* keyset pagination for the invocations list ([#1410](https://github.com/rolter-ai/rolter/pull/1410)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* list every project in an org in one request ([#1443](https://github.com/rolter-ai/rolter/pull/1443)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* warn about unrecognised rolter.toml keys at startup ([#1438](https://github.com/rolter-ai/rolter/pull/1438)) by [@ormeilu](https://github.com/ormeilu)
- *(core)* stability marker so experimental subsystems are labelled [#1385] ([#1402](https://github.com/rolter-ai/rolter/pull/1402)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* static credentials and per-server transport settings for mcp [#952] ([#1349](https://github.com/rolter-ai/rolter/pull/1349)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* label primitive for providers, routes, groups and models [#985] ([#1330](https://github.com/rolter-ai/rolter/pull/1330)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* totp second factor for local accounts [#1078] ([#1324](https://github.com/rolter-ai/rolter/pull/1324)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* update an SSO provider in place instead of delete-and-recreate [#1233] ([#1299](https://github.com/rolter-ai/rolter/pull/1299)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* serve an OpenAPI document for the control-plane API [#1040] ([#1310](https://github.com/rolter-ai/rolter/pull/1310)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* export the live configuration as importable rolter.toml [#1082] ([#1311](https://github.com/rolter-ai/rolter/pull/1311)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* cached update check with a dashboard hint and a cli notice [#902] ([#1294](https://github.com/rolter-ai/rolter/pull/1294)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* per-budget override for unpriced_policy [#996] ([#1286](https://github.com/rolter-ai/rolter/pull/1286)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* make payload capture discoverable, on in the dogfood profile ([#1287](https://github.com/rolter-ai/rolter/pull/1287)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* give the curated MCP library entries their tool and scope lists ([#1284](https://github.com/rolter-ai/rolter/pull/1284)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* filter /analytics/invocations by business unit and customer ([#1281](https://github.com/rolter-ai/rolter/pull/1281)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* require a name and an expiry when minting a virtual key [#945] ([#1172](https://github.com/rolter-ai/rolter/pull/1172)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* carry the security policy to the gateway and enforce it [#1162] ([#1165](https://github.com/rolter-ai/rolter/pull/1165)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* throttle and audit failed logins [#1079] ([#1161](https://github.com/rolter-ai/rolter/pull/1161)) by [@ormeilu](https://github.com/ormeilu)

### Refactoring
- *(core)* compute money in exact decimal instead of f64 ([#1450](https://github.com/rolter-ai/rolter/pull/1450)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* resolve the public base url once, not per request ([#1435](https://github.com/rolter-ai/rolter/pull/1435)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* extract the guardrail repo into its own module [#1042] ([#1320](https://github.com/rolter-ai/rolter/pull/1320)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* extract mcp repo into its own module [#1042] ([#1303](https://github.com/rolter-ai/rolter/pull/1303)) by [@ormeilu](https://github.com/ormeilu)

### Testing
- *(control)* send the admin token to config problems in the example-key test ([#2488](https://github.com/rolter-ai/rolter/pull/2488)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* drive ui events through clickhouse end to end [#1728] ([#1749](https://github.com/rolter-ai/rolter/pull/1749)) by [@ormeilu](https://github.com/ormeilu)
- *(store)* give every worktree its own postgres test database [#1430] ([#1734](https://github.com/rolter-ai/rolter/pull/1734)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* assert the config export is clean under the key lint [#1439] ([#1472](https://github.com/rolter-ai/rolter/pull/1472)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* assert the totp replay rule, not the clock ([#1452](https://github.com/rolter-ai/rolter/pull/1452)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* drop the per-test schema when an integration test finishes ([#1428](https://github.com/rolter-ai/rolter/pull/1428)) by [@ormeilu](https://github.com/ormeilu)
## [0.0.11](https://github.com/rolter-ai/rolter/compare/rolter-control-v0.0.10...rolter-control-v0.0.11) - 2026-08-13

### Bug Fixes
- *(control)* distinguish an open-mode 401 from a missing session ([#982](https://github.com/rolter-ai/rolter/pull/982)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* make the base-URL hint kind-aware and preview the resolved path ([#981](https://github.com/rolter-ai/rolter/pull/981)) by [@ormeilu](https://github.com/ormeilu)
- *(ui)* drive the currency chooser from the configured rate table ([#978](https://github.com/rolter-ai/rolter/pull/978)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* serve a partial snapshot instead of withholding the fleet ([#973](https://github.com/rolter-ai/rolter/pull/973)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* refuse open mode on a non-loopback bind [#970] ([#971](https://github.com/rolter-ai/rolter/pull/971)) by [@ormeilu](https://github.com/ormeilu)
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
- *(ui)* give the dashboard a browser tab icon ([#979](https://github.com/rolter-ai/rolter/pull/979)) by [@ormeilu](https://github.com/ormeilu)
- *(gateway)* record unpriced traffic instead of billing it at zero ([#975](https://github.com/rolter-ai/rolter/pull/975)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* test a provider's endpoint and credential from the dashboard ([#935](https://github.com/rolter-ai/rolter/pull/935)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* predicted-latency scheduling ([#912](https://github.com/rolter-ai/rolter/pull/912)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* instrument snapshot generation and the CRUD API [#845] ([#909](https://github.com/rolter-ai/rolter/pull/909)) by [@ormeilu](https://github.com/ormeilu)
- *(balancer)* add lora-aware scheduling strategy [#853] ([#896](https://github.com/rolter-ai/rolter/pull/896)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* add ldap identity provider with group mapping [#241] ([#886](https://github.com/rolter-ai/rolter/pull/886)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* render OTel Collector config from connectors [#836] ([#870](https://github.com/rolter-ai/rolter/pull/870)) by [@ormeilu](https://github.com/ormeilu)
- *(auth)* add pluggable IdentityProvider trait [#239] ([#865](https://github.com/rolter-ai/rolter/pull/865)) by [@ormeilu](https://github.com/ormeilu)
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
- *(core)* eliminate intermediate string join allocations [ROL-PERF] ([#862](https://github.com/rolter-ai/rolter/pull/862)) by [@ormeilu](https://github.com/ormeilu)
- *(control)* use HashSet for role deduplication [ROL-PERF] ([#859](https://github.com/rolter-ai/rolter/pull/859)) by [@ormeilu](https://github.com/ormeilu)
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
