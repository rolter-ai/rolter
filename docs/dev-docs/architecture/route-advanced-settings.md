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

| Field                                       | Read in                                                                      | Notes                                                                                                                         |
| ------------------------------------------- | ---------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `visibility`                                | `state.rs` (`RouteEntry::in_tenancy_of`), `handlers.rs` (`model_visible_to`) | tenancy and key access                                                                                                        |
| `guardrails`                                | `state.rs` (`resolve_selection`)                                             | per-route rule selection, #590                                                                                                |
| `limits.output_tokens`                      | `handlers.rs` (`proxy`)                                                      | refuses `max_tokens` above it with `400 max_tokens_exceeded`                                                                  |
| `limits.timeout_secs`                       | `rolter-proxy` (`Forwarder::request_budget`)                                 | replaces `[timeouts].request_secs` for the call, time to response headers only                                                |
| `limits.retries`                            | `handlers.rs` (`ModelLimits::max_retries`, three forward loops)              | replaces `[retry].max_retries`; `0` is a value; applied up to `MAX_ROUTE_RETRIES` (10)                                        |
| `headers`, `locked_headers`                 | `rolter-proxy` (`RouteHeaders::apply`)                                       | static upstream headers; see below                                                                                            |
| `model_type`, `capabilities`, `description` | the dashboard                                                                | read back by the model sheet to choose which parameters and prices it offers; the gateway does not enforce them or serve them |

Whether `GET /v1/models` should serve the metadata is a product call, tracked in
#2939; the description is free text written "for the team" and may not be public.

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

## What was removed, and why

| Field                                                                                                 | Why it is not a route setting                                                                                                                                                                                                                                                           |
| ----------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `base_url`                                                                                            | The provider's key travels with the request. A route that named its own host would send an org-wide provider's credential wherever a project admin pointed it, bypassing the host pin a hosted kind carries (`allow_custom_api_base`, ADR-0029). A second endpoint is a second provider |
| `insecure_tls`                                                                                        | There is no provider-level equivalent, and a route is the wrong owner for turning verification off. A private CA is `[tls].ca_bundles` or the provider's `ca_bundles`                                                                                                                   |
| `limits.rpm`, `limits.tpm`                                                                            | The limiter (`rate_limits.rs`) is keyed by org, team, project, key, business unit or customer, with Redis counters. A route scope is a design of its own: a new `BudgetScope` and its storage, or a second keying, plus what a replica without Redis does (#2936)                       |
| `limits.concurrency`                                                                                  | No per-route in-flight cap exists. `LoadTracker` counts in-flight per target for balancing and is not an admission check (#2936)                                                                                                                                                        |
| `limits.context_window`                                                                               | Nothing could enforce it: a prompt's token count is not known before the upstream tokenizes it                                                                                                                                                                                          |
| `pricing.image_per_unit`, `audio_input_per_minute`, `audio_output_per_minute`, `cache_write_per_mtok` | Cost is computed from token counts (`ModelPriceConfig::cost`). Per-image and per-minute pricing needs a unit count out of the response and a price row to carry it (#2937)                                                                                                              |

None of this needs a migration. None of the config types deny unknown fields, so a
stored blob that carries a removed key still deserializes whole, the key ignored.
The store keeps the raw JSON it is sent, so `set_route_advanced` strips the removed
keys first (`strip_retired_advanced_keys`) and the dashboard's `advancedToApi`
sheds them too. A `rolter.toml` that sets one is reported by the unknown-key lint.
