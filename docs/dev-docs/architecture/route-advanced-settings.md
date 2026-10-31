# Route advanced settings

A route's `advanced` block is `rolter_core::AdvancedModelConfig`. It is stored in
`routes.advanced` (jsonb), edited through `PUT /api/v1/routes/{id}/advanced`, and
shipped in every snapshot. This page is the record of which of its fields the
gateway reads, where, and why the others are gone (#2924).

## The rule

A field in this block is either read by something or it does not exist. Before
#2924 it held a dozen settings that were validated, stored, put in every snapshot
and read by nothing, while the dashboard and the docs described them as working.
The same shape as `additional_fields` (#1665) and the route-level
`pricing.cache_write_per_mtok` (#2890). A new field needs a reader and a test that
fails when the reader's line is removed; the control plane's validation and the
dashboard follow the reader, never the other way round.

## What is read

| Field                                          | Read in                                                                                                     | Notes                                                                                                                                                                                    |
| ---------------------------------------------- | ----------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `visibility`                                   | `state.rs` (`RouteEntry::in_tenancy_of`), `handlers.rs` (`model_visible_to`)                                | tenancy and key access                                                                                                                                                                   |
| `guardrails`                                   | `state.rs` (`resolve_selection`)                                                                            | per-route rule selection, #590                                                                                                                                                           |
| `limits.output_tokens`                         | `handlers.rs` (`RequestCtx::output_token_cap`)                                                              | refuses `max_tokens` above it with `400 max_tokens_exceeded`                                                                                                                             |
| `limits.rpm`, `limits.tpm`                     | `handlers.rs` (`route_limit_refusal`), `rate_limits.rs` (`check_route`)                                     | per-route Redis window, see [Route throughput limits](#route-throughput-limits)                                                                                                          |
| `limits.concurrency`                           | `handlers.rs` (`route_limit_refusal`), `route_limits.rs`                                                    | per-replica in-process permit held on the `LoadGuard`                                                                                                                                    |
| `limits.timeout_secs`                          | `rolter-proxy` (`Forwarder::request_budget`)                                                                | replaces `[timeouts].request_secs` for the call, time to response headers only                                                                                                           |
| `limits.retries`                               | `handlers.rs` (`ModelLimits::max_retries`, the two forward loops, `forward_classic` and `forward_variants`) | replaces `[retry].max_retries`; `0` is a value; applied up to `MAX_ROUTE_RETRIES` (10)                                                                                                   |
| `headers`, `locked_headers`                    | `rolter-proxy` (`RouteHeaders::apply`)                                                                      | static upstream headers; see below                                                                                                                                                       |
| `model_type`, `capabilities`, `context_length` | the dashboard, `handlers.rs` (`add_model_metadata`)                                                         | `GET /v1/models` serves them on a route's entry when set (#2939); advertised, never enforced. `context_length` is `>= 1`, checked by `AdvancedModelConfig` validation and the routes API |
| `description`                                  | the dashboard                                                                                               | free text written "for the team", may hold internal routing notes, so it is never served to API clients                                                                                  |

## How a route reaches the forwarder

`Snapshot::from_config` builds a `RouteEntry` per route, and with it a
`rolter_proxy::RouteOverrides` (`RouteOverrides::from_advanced`): the timeout as a
`Duration` and the headers parsed into `HeaderName`/`HeaderValue` once. The
request path never parses a header. A synthetic `provider-slug/model` or
`group-slug/model` entry has no route and carries the empty default.

`proxy` and `proxy_multipart` pass `&entry.overrides` to
`ProviderQueues::forward_json` / `forward_raw`, which carry it in the queued `Job`
and hand it to `Forwarder::forward_json_with` / `forward_raw_with`. The unsuffixed
`Forwarder` methods remain, for the callers that have no route (health probes,
the semantic cache's embedding call, stored-response lifecycle calls), and pass
`RouteOverrides::default()`.

The retry budget is not part of `RouteOverrides`: the loops live in the handlers
and read `entry.route.advanced.limits.max_retries(snap.retry.max_retries)` once
per request.

## Headers

`RouteHeaders::apply` runs last on the request builder and uses
`RequestBuilder::headers`, which **replaces** a header of the same name set
earlier (`header` appends). That is what makes a route header win over a
deployment-wide injected one, and a locked one win over a caller's.

A caller reaches the upstream only with the trace context, the `anthropic-*`
vendor namespace and the names in `[client].forwarded_headers`
(`trace::outbound_headers`). So a lock only ever matters for a header in that set:
an unlocked route header is skipped when the caller sent one of that name, a
locked one replaces it.

`rolter_core::RESERVED_ROUTE_HEADERS` is refused everywhere a header can enter,
by the same predicate (`AdvancedModelConfig::header_problems`):

- the control plane (`validate_advanced`) answers `400`;
- `GatewayConfig::validate` reports it for a file config (the gateway logs it at
  startup) and `rolter check` fails on it (`route_header_findings`);
- `sanitize_for_snapshot` drops the offending header from that route and reports
  it, so one bad row cannot 500 `/internal/snapshot` for every tenant;
- `RouteHeaders::compile` skips it, the last line for a row that reached the
  gateway some other way.

The set is every provider kind's credential header (`ProviderKind::auth_header`)
plus the framing and hop-by-hop headers. A route is edited by a project admin and
a provider's key by an org admin, so a route that could replace `authorization`
would let the first present a credential of their own in place of the second's.

## A stored blob that does not parse

The control plane parses every write through `AdvancedModelConfig`, so a blob that
does not deserialize reached `routes.advanced` another way (SQL, a seed) or no
longer fits a type that changed. The loader does not read it as
`AdvancedModelConfig::default()`: that is public visibility, no allow-lists, no
guardrail override and no limits, so a route restricted to two teams would be
served to every key of its org, and nothing would say so (#2938). The route is left
out of the snapshot instead and `GET /api/v1/config/problems` names it and the
column (`route_settings_from_row`); the other routes keep serving. `params` and
`param_policy` are read by the same function and fail the same way, since their
defaults are just as permissive (see
[config propagation](config-and-hot-reload.md)). Removing a field is safe for this
reason: unknown keys are ignored, so only a value of the wrong type fails.

## Route throughput limits

`limits.rpm`, `limits.tpm` and `limits.concurrency` came back in #2936, each with
a reader. The design questions the issue asked, and what was decided:

| Question                                           | Decision                                                                                                                                                                                                                                                                                                                                  |
| -------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| A new `BudgetScope::Route`, or route-owned limits? | Route-owned `advanced.limits`. A `BudgetScope` names a caller, lives in its own table and screen, and would put a route's cap in two places. The cap follows the route row, is edited on the model sheet and ships in the snapshot with it. The limiter learned a second keying (`RouteWindow`); the scope-chain code is unchanged        |
| The Redis key must include the org                 | `rolter:rl:route:<org>:<route>:<kind>:<bucket>` (`kind` is `req` or `tok`), the org being the route's own `tenancy.org_id` (empty for a route from a gateway config file). The route's tenancy, not the caller's org, so one counter serves every caller of the route                                                                     |
| Is `tpm` recorded after the response?              | Yes, acceptable, and documented. Same reactive rule as every scope: refused when the window already holds the cap. `TokenRecorder` carries the route's window next to the caller's, so streamed, non-streamed and realtime turns all land in it                                                                                           |
| No Redis: fail open?                               | `rpm` and `tpm` fail open exactly like the scope chain (same script, same `fail_open` counter), so a cap is advisory on a gateway without Redis. `concurrency` never needed Redis and is enforced in process                                                                                                                              |
| `concurrency`: permit, status, queue, scope        | An RAII `ConcurrencyPermit` riding on the successful attempt's `LoadGuard`, so it is held to the last byte of a streamed body and released when a disconnect drops the stream. Refused with `429` and `Retry-After: 1`; no queue, because a queue hides overload and holds sockets. Counted **per replica**, which the docs state         |
| Where in the pipeline?                             | After route resolution, the complexity-tier swap, `authorize_route` and the `output_tokens` check, before templates and guardrails; `proxy`, `proxy_multipart` and the realtime upgrade share `route_limit_refusal`. `concurrency` is taken first, so a request turned away for a slot takes no `rpm` slot                                |
| What does a refusal cost the caller?               | The caller's own `rpm` is charged earlier (`rate_limit_refusal` runs before a route exists, and moving it would reorder every refusal), so a request a route refuses has still taken a slot of the key's `rpm`. Stricter, not looser, and documented                                                                                      |
| Stale stored values                                | Migration `0090_clear_unread_route_limits.sql` removes `rpm`, `tpm` and `concurrency` from stored routes: a number typed into the old form while nothing read it must not begin refusing traffic on upgrade. Enforcement is opt-in from this release                                                                                      |
| Zero, or out of range                              | Refused on write (`validate_advanced`, `1..=10000000` for `rpm` and `concurrency`, `>= 1` for `tpm`) and by `GatewayConfig::validate` for a file. A stored row that carries one has it pruned in `sanitize_for_snapshot` with a problem line, so the route serves without that cap instead of refusing everything or 500-ing the snapshot |
| Metrics                                            | `rolter_route_limit_blocks_total{route,kind}` (`kind` is `rpm`, `tpm` or `concurrency`), and the existing `rolter_rate_limit_blocks_total` still totals every throughput refusal. The route label is bounded by the number of configured routes                                                                                           |

The `RouteWindow` is built once per route in `Snapshot::from_config`
(`RouteEntry::throughput`), so the request path formats no key. A route that sets
no cap returns from `route_limit_refusal` before it reads anything. A refused
request answers through the same `ApiError` as a key limit, so the OpenAI and
Anthropic surfaces share one envelope (`error.type = rate_limit_error`, code
`rate_limit_exceeded`) and `Retry-After`.

Not covered: `provider-slug/model` and `group-slug/model` addressing has no route,
so no cap applies; the built-in `fake-llm` model has none either.

## What was removed, and why

| Field                                                                                                 | Why it is not a route setting                                                                                                                                                                                                                                                           |
| ----------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `base_url`                                                                                            | The provider's key travels with the request. A route that named its own host would send an org-wide provider's credential wherever a project admin pointed it, bypassing the host pin a hosted kind carries (`allow_custom_api_base`, ADR-0029). A second endpoint is a second provider |
| `insecure_tls`                                                                                        | There is no provider-level equivalent, and a route is the wrong owner for turning verification off. A private CA is `[tls].ca_bundles` or the provider's `ca_bundles`                                                                                                                   |
| `limits.context_window`                                                                               | Nothing could enforce it: a prompt's token count is not known before the upstream tokenizes it                                                                                                                                                                                          |
| `pricing.image_per_unit`, `audio_input_per_minute`, `audio_output_per_minute`, `cache_write_per_mtok` | Cost is computed from the model's price row, never a route. #2937 moved the per-unit rates there as `model_prices.image_per_unit`, `audio_input_per_minute` and `speech_per_mchar`; there is no audio-output rate, since a speech response states no length                             |

None of this needs a migration. None of the config types deny unknown fields, so a
stored blob that carries a removed key still deserializes whole, the key ignored.
The store keeps the raw JSON it is sent, so `set_route_advanced` strips the removed
keys first (`strip_retired_advanced_keys`) and the dashboard's `advancedToApi`
sheds them too. A `rolter.toml` that sets one is reported by the unknown-key lint.
