# MCP servers, OAuth grants and sessions

rolter stores the OAuth state behind Model Context Protocol access: which MCP servers an org has registered, which users consented to what, and the token sessions held against those consents. Transport and the authorization-code / on-behalf-of exchange belong to the MCP proxy; this layer owns persistence, listing, revocation and audit, so it is useful to a stdio, SSE, streamable-HTTP or WebSocket implementation alike.

## Model

```
mcp_servers (org-scoped)
   ├── mcp_oauth_grants   one live grant per (server, user)
   │      └── mcp_oauth_sessions   token material, sealed at rest
   ├── mcp_tool_groups    named server/tool policy manifests
   └── mcp_gateway_settings   organization defaults
```

- **Server** — name, slug (unique per org), URL, transport and the OAuth scopes every proxied call requires. Deleting one cascades to its grants and sessions: withdrawing the server withdraws access to it.
- **Grant** — a user's consent against a server, with the scope set they agreed to. A user holds at most one _live_ grant per server (a partial unique index on `revoked_at is null`); revoked grants are kept so the audit trail survives. Re-consenting updates the scopes in place rather than accumulating rows.
- **Session** — the tokens issued under a grant, with `expires_at`, an optional refresh token and `refresh_expires_at`.

## Token handling

Resource-identifier parse failures redact URL credentials at the parser boundary, including unsupported schemes and missing hosts. Every OAuth caller receives the same safe diagnostic; consent, refresh and token exchange also redact errors before forwarding them.

Access and refresh tokens are sealed with AES-256-GCM under the deployment KEK (`ROLTER_KEK`), the same mechanism as upstream provider credentials — there is deliberately no plaintext column for either. Ciphertext and nonce sit side by side; the KEK never reaches the database.

The store exposes tokens through two credential-only paths. `McpOAuthRepo::open_session` opens one explicitly selected live session for lifecycle code. The Postgres config store opens only the newest live session per `(server, user)` whose scopes remain within its live grant and cover the server's required scopes; those records travel through the token-guarded `/internal/snapshot` channel already used for decrypted provider credentials. Public API DTOs carry only metadata and a `has_refresh_token` boolean.

The gateway indexes servers by `(org, slug)` and sessions by `(server, user)`. A request to `/mcp/{server}` must authenticate with a database-backed virtual key whose `created_by` user owns the selected session. The gateway repeats the required-scope check before connecting and replaces the caller's virtual key with the downstream bearer token. Revocation, expiry and policy writes bump `config_version`, so snapshot polling removes authorization without a restart.

## Who sees what

| caller                   | grants / sessions visible | may revoke     |
| ------------------------ | ------------------------- | -------------- |
| superadmin / admin token | every one in the org      | any            |
| org admin                | every one in the org      | any            |
| org member or viewer     | only the ones they own    | only their own |
| anyone outside the org   | none (`403`)              | none (`403`)   |

Every listing is joined through `mcp_servers.org_id`, so a cross-tenant read is not expressible, not merely filtered out.

## The curated catalog

`LIBRARY` in `crates/rolter-control/src/mcp_oauth.rs` holds the four reviewed
server definitions the dashboard offers under **MCP Library**: GitHub, Sentry,
Notion and Linear. Each entry names the server's URL, transport, tool manifest
and the OAuth scopes an authorization request must ask for.

The manifest matters beyond the card it renders on. `create_server` admits a
`source: "library"` request only when it reproduces the catalog entry element
for element, so what is written here is exactly what an installed server ends
up storing — and therefore what the tool tally counts, what a tool group can
select, and what `allow_unlisted_tools` is deciding about. While the lists were
empty (#1252) an install produced a server that declared nothing at all.

Two properties are worth stating plainly:

- **The lists are the fallback, not the source of truth.** An installed server
  is asked for its tools with `tools/list` (see
  [Tool discovery](#tool-discovery)) and what it answers replaces this list on
  the row. The snapshot is what a server carries until then, and what stays if
  the server cannot be reached; nothing re-checks it against the live server on
  its own, so a tool renamed upstream is stale here until someone edits the
  file.
- **An empty `required_scopes` can be the accurate answer.** Notion's remote
  server grants access to the pages the user picks on its consent screen rather
  than to named scopes, so it asks for none. Every other entry declaring an
  empty list would be an unfilled row, and a test enforces that distinction.

## Tool discovery

`crates/rolter-control/src/mcp_discovery.rs` asks a registered server what
tools it has, through the small client in `mcp_client.rs` (`initialize`,
`notifications/initialized`, then `tools/list` following `nextCursor`; JSON or
SSE replies). The answer is written back to the server row (#1283):

- **When.** Best effort and never blocking: in the background after a server is
  installed, after a `bearer`/`header` credential is set, and after an OAuth
  consent produces a session; and on demand with
  `POST /api/v1/mcp-servers/{id}/discover`.
- **As whom.** A static-credential server is asked with its credential. A server
  behind OAuth is asked with the _caller's own_ live session on it, so discovery
  there is per grant and borrows nobody else's token. With no session the row
  stays `pending` with `tools_discovery_error = needs_session`.
- **Reconciliation.** Discovered tools come first, in the server's order. A tool
  the row already had that the server no longer returns is **kept** and listed
  in `tools_stale`, never pruned, so a tool group that names it still validates
  and an operator decides what to do. A tool that returns is unflagged.
- **Status.** `tools_discovery_status` is `pending` (never answered, so `tools`
  is the catalog's fallback or whatever the operator typed), `discovered` (the
  server answered, possibly with no tools, which is "declares no tools") or
  `failed` (a later attempt got no answer; the last manifest is kept).
  `tools_discovery_error` carries a bounded code (`unreachable`,
  `auth_rejected`, `handshake_failed`, `egress_denied`, `needs_session`,
  `credential_unavailable`), never the upstream's text. `tools_discovered_at`
  is the last success.
- **Egress.** The address is checked against the egress policy before any
  connection and again at connect time, and no redirect is followed, exactly as
  for the OAuth client.
- **Schemas.** Descriptions and input schemas are stored beside the manifest and
  served by `GET /api/v1/mcp-servers/{id}/tools`, not by the listing.

Both HTTP transports are spoken (#3299). A `streamable_http` server is asked with
`POST`s whose reply is JSON or an SSE body. A server registered as `sse` (legacy
HTTP+SSE) gets the older handshake in `McpClient::open_legacy_stream`: `GET` the
registered address as an event stream, wait for the `endpoint` event, `POST` every
message there and read each reply off the stream, since the `POST`s answer `202`.
The endpoint has to be on the origin the server was registered with (otherwise the
handshake fails and nothing is sent to it), so a server cannot steer the control
plane to an address the egress policy never checked. Discovery, the connection
probe and the try-it call all go through the client, so all three follow the
server's `transport`.

## Connection status and try-it

`crates/rolter-control/src/mcp_probe.rs` (#951) lets an operator see whether a
server works, and run its tools, without reading gateway logs. Both endpoints
need the capability to update the server (`mcp_server` / `update`), since a tool
call has consequences and spends the operator's credential; both speak MCP
through the same client and egress rules as discovery, and a server behind OAuth
is spoken to with the _caller's own_ session, so the admin token and open mode
(which are nobody) get `needs_session`.

- `POST /api/v1/mcp-servers/{id}/test` runs the `initialize` handshake and
  answers `status`: `reachable`, or the failure it was — `unreachable` (no
  answer: refused, DNS, timeout), `handshake_failed` (it answered, not as an MCP
  server does), `auth_rejected` (`401`/`403`), `egress_denied` (policy refused
  the address), or `needs_session` / `credential_unavailable` when there was
  nothing to authenticate with. It also returns `reachable`,
  `handshake_completed`, the negotiated `protocol_version`, the server's name
  and version and the latency.
- `POST /api/v1/mcp-servers/{id}/tools/call` takes `{tool, arguments}` and
  returns the raw JSON-RPC `request` and `response`. The tool must be in the
  server's manifest and `arguments` an object. `outcome` is `success`,
  `tool_errored` (the server answered and the tool reported `isError` or a
  JSON-RPC error: a fifth state, because the server did answer), or a failure
  state above. A reply over 256 KiB is returned as a marked preview. The call is
  audited as `mcp_server.tool_call` with the tool and the outcome, never the
  arguments, and does not appear in the MCP tool-call log because it does not go
  through the gateway.
- **What the row remembers** (migration `0115`): `last_contact_at`,
  `last_protocol_version` and `last_server_name` from the last completed
  handshake, and `last_error_kind` / `last_error_at` from the last failed one.
  Discovery records them too. The failure is **kept after a later success**, so
  a status can say both "reachable" and "it last broke at". Which is current is
  whichever of `last_contact_at` and `last_error_at` is later.
- **`config_version`.** `0115` also narrows the `mcp_servers` update trigger to
  the columns the gateway reads (`org_id`, `slug`, `url`, `transport`,
  `enabled`, `tools`, `required_scopes`, the auth and credential columns and the
  three transport overrides); insert and delete still bump. Probes, discovery
  bookkeeping and OAuth-client edits therefore no longer wake every gateway for a
  snapshot that did not change. A new column the snapshot starts reading must be
  added to that list.

## Tool groups as an access boundary

The gateway enforces enabled tool groups (#1403) in `crates/rolter-gateway/src/mcp_policy.rs`,
after the caller is authenticated and before anything is forwarded.

- **Snapshot.** Enabled groups travel as `mcp_tool_groups`; `mcp_tool_groups`
  carries a `bump_config_version()` statement trigger (migration `0113`), so an
  edit reaches a running gateway. The gateway indexes them into
  `Snapshot::mcp_governed_tools`, server id to allowed tool names, counting only
  references whose server is in the group's own organization.
- **Rule.** A server named by at least one enabled group is governed: a
  `tools/call` is forwarded only for a tool in the union of those groups, and
  anything else (including a call that names no tool) is refused `403`
  `mcp_tool_denied`. A server no group names is not governed here. "The caller's
  effective groups" is the organization's enabled set unless the caller has an
  assignment (below).
- **Assignments (#3295).** `mcp_tool_group_assignments` (migration `0150`, with
  the `bump_config_version()` trigger in `0151`) assigns a group to a
  `virtual_key`, `project` or `team`. The subject has no foreign key, since it is
  polymorphic: the control plane refuses one outside the group's organization
  when it is written, and three statement-level `after delete` triggers on
  `virtual_keys`, `projects` and `teams` (migration `0200`, #3382, the pattern
  of the labels sweep) delete the subject's rows in the deleting transaction,
  so a deleted subject never lingers in the assignments listing or the snapshot
  and the sweep bumps `config_version` through the table's own trigger.
  Assignments travel as `McpToolGroupConfig::assignments`. A disabled group
  travels too, with its tools and assignments and `disabled: true`. It governs no
  one else (`mcp_governed_tools` counts enabled groups only), but the callers
  assigned to it stay narrowed to none of its tools on every server it names,
  even when it is the only group that names one (`Snapshot::mcp_group_servers`
  records the servers of every group), instead of falling back to the
  organization-wide set or the whole server; deleting a group removes its assignments with it
  (cascade), and its callers return to the organization-wide set. The gateway
  indexes them as `Snapshot::mcp_group_assignments` (subject id to
  `(kind, organization, group)`) and each enabled group's tools as `mcp_group_tools`. `Snapshot::build` also merges, once, the union a
  database key's own, project's and team's groups allow on each server
  (`Snapshot::mcp_caller_tools`, #3435; keys with the same groups share one
  `Arc`), so a request borrows it with two lookups. Resolving it per request
  cost a `Vec` of the caller's groups plus a `String` clone and a set insert
  for every tool of every group assigned to the caller, on a path meant to
  allocate nothing (for three groups of 50 tools each, one `Vec`, about 150
  `String` clones and as many set inserts per request); now it costs none of that. A caller the snapshot built
  nothing for (no assignment, or not a key of the snapshot) still takes the
  merge, `Rules::governed_merged`, which is also what a test holds the
  precomputed union to.
  `Caller::assigned_groups` collects the groups assigned to the caller's key,
  project and team and returns at once when nothing is assigned anywhere; with
  none the organization-wide set applies, otherwise the allowed tools on a
  governed server are the union of those groups' tools there, which is empty
  when none names it. The union stays within the organization's enabled groups,
  but adding another assignment can grant an already-assigned caller more tools;
  it is not a monotonic restriction. An empty table changes nothing. The same
  set drives the `tools/list` filter. API:
  `GET /api/v1/orgs/{org_id}/mcp/tool-group-assignments` and
  `PUT /api/v1/mcp/tool-groups/{id}/assignments` (replaces the group's whole set
  in one transaction; a stored row whose subject is gone, which the `0200` sweep
  leaves only to a write racing the delete or to a row from before it, is dropped
  rather than refused), under the `mcp_tool_group_assignment` capability. The create body
  takes `assignments` too and writes them in the group's own transaction, so a
  group meant to be narrowed is never visible as organization-wide. The
  dashboard's group dialog edits all three kinds (the key picker reads the
  organization's keys in one request, `GET /api/v1/orgs/{org_id}/virtual-keys`,
  and groups them by `project_id`, #3380, #3467) and mounts its editor only once the stored assignments
  are read, so it can never save an empty set over them; a stored key is dropped
  only once the organization's keys were read and it is not among them. Both snapshot fields added for assignments and the manifest default
  restrict what the gateway serves, which ADR-0038 would call a format bump; they
  stay in format 1 because everything before the 1.0.0 tag does, and the shape
  fixture carries them.
- **Parsing.** When the rule applies the body is parsed in full, as one message
  or a batch, with no substring shortcut: `tools\/call` and `\u`-escaped spellings
  are the same JSON string, so a byte search would be a bypass. A batch is
  refused whole if any call in it is. A `tools/call` notification (no id) is
  refused too. A body that is not JSON is refused `400`
  `mcp_request_unreadable`; an empty body is not a call and is forwarded.
- **Encoded bodies (#3475).** The relay does not decode a request body, and
  `Content-Encoding` is end to end, so an upstream that honours it would run a tool
  the policy never saw. A `POST` to a governed server whose `Content-Encoding` is
  anything but `identity` (in any header line or comma-separated value) is refused
  `415` `mcp_request_unreadable` before it is parsed, like any other body the
  relay cannot judge (see the failure policy below). There is deliberately no decoder:
  MCP clients do not compress requests, and one on this path would be a
  decompression-bomb surface for a shape nobody sends. A server no group or manifest
  governs is not affected.
- **Repeated keys (#3298).** `serde_json` keeps the last copy of a repeated key
  and another parser may keep the first, and a Go decoder (`encoding/json`, so
  mcp-go, go-sdk and the GitHub MCP server) matches keys case-insensitively and
  keeps the last. A message, or its `params`, that repeats a key once ASCII case
  is folded (plus U+017F to `s` and U+212A to `k`) is refused `403`
  `mcp_tool_denied`, and the refused
  `tools/call` requests are logged like any other refusal (#3381). For the same
  reason the policy reads `method`, `params`, `name` and `id` the way such a
  decoder would, whatever their case, so a lone `Method` is judged rather than
  ignored. Keys inside tool `arguments` are the tool's own data and are not
  checked. The body is parsed once, building the `Value` and noting repeats in the
  same pass (`Parse`).
- **`tools/list` (#3297).** A reply that lists tools is narrowed by the same
  `judge` rule (`mcp_list_filter.rs`). It is recognised by its shape, a
  `result.tools` array, and not by its id: ids compare as JSON values (`1` is not
  `1.0`), may be `null` or absent, and a Go decoder reads `ID` as `id`, so
  matching replies to requests could be bypassed. The filter is built only when a
  rule applies to the server and the request holds a `tools/list` (any id), or
  for a `GET` stream, which can carry the reply to a request posted elsewhere (a
  legacy `sse` server answers every `POST` with `202` and sends the reply on its
  stream). `202`, `204` and empty bodies pass through untouched. A JSON reply is
  buffered (16 MiB at most), an SSE reply is rewritten event by event
  (`EventScanner` ends an event at any empty line, `\n`, `\r\n` or `\r`, and
  resumes where it stopped; an event over 16 MiB cuts the stream), and each page
  of a paginated listing is filtered on its own with its `nextCursor` kept. The
  request goes out with `Accept-Encoding: identity`; a reply that is still
  encoded, too large or not JSON is refused `502` rather than forwarded
  unfiltered.
- **Logging.** Each refused call becomes an `auth_denied` row in
  `mcp_tool_call_logs` with the tool name and the caller's attribution, via
  `PendingCall::from_request`; the closed status set is unchanged.

## Organization settings in the proxy

The proxy applies `mcp_gateway_settings` (#1404). Saved rows travel in the
snapshot as `mcp_settings` (migration `0114` adds the `bump_config_version()`
trigger); an organization with no row is absent and gets the defaults.

- **Precedence** for connect timeout, request timeout and retries: the server's
  own column, then the organization's row, then the deployment-level default the
  forwarder uses for `None` (`transport_overrides` in `mcp_proxy.rs`).
- **Declared manifest.** `McpServerConfig::tools` carries the manifest. A server
  with a non-empty one serves only those tools when the organization's saved
  settings have `allow_unlisted_tools` off; an empty manifest checks nothing.
  An organization with no settings row follows its `orgs.mcp_enforce_manifests`
  marker (#3334): one that existed before the change allows unlisted tools, and
  migration `0137` set every row saved before enforcement to allow them (the
  column was stored but never read until #1404, so an upgrade must not start
  refusing calls); one created afterwards enforces. Migration `0152` adds the
  marker with default `false` for the rows that exist and then switches the
  default to `true`, so no org-creation path needs to know about it. It is a
  column on `orgs` and not on `mcp_gateway_settings` because inserting a settings
  row at org creation would freeze its timeouts and retries at that day's values
  instead of letting them follow the deployment defaults. It reaches the gateway
  as `McpServerConfig::enforce_manifest_by_default` (read in `load_mcp_servers`,
  with a column-scoped `bump_config_version()` trigger in `0153`); a saved row
  still decides for itself, which is how an admin opts out. The settings read
  for an org with no row reports `allow_unlisted_tools` from the same marker.
  This sits beside the tool-group rule and both must pass.
- **Failure policy (#3527).** The relay forwards only a body it has judged. On a
  governed server (`Rules::any`), `mcp_proxy` strips a leading UTF-8 byte-order
  mark (`mcp_policy::without_bom`; RFC 8259 lets a parser ignore one), judges the
  rest and forwards the stripped bytes, so the upstream receives exactly what was
  judged. Any other body `serde_json` rejects is `Read::Unreadable`, and
  `evaluate_read` answers it `Verdict::Unreadable`, `400`
  `mcp_request_unreadable`: UTF-16 and UTF-32 (RFC 8259 requires UTF-8 between
  systems), `NaN` and `Infinity`, trailing commas and comments, nesting past
  `serde_json`'s recursion limit. Each is a parser differential: Python's
  `json.loads` takes `NaN` and, on bytes, auto-detects UTF-16/32, and the Python
  `mcp` SDK 2.3.0 ran a `tools/call` carrying `NaN` for a tool the organization
  refused when the relay forwarded it under `fail_open`. MCP clients send strict
  JSON, so an honest client loses nothing. The organization setting
  `fail_open` (`default_failure_mode` in the API, the config file and
  `mcp_gateway_settings`) used to forward such a body unchecked, which broke its
  own promise never to let a refused tool through. It is **deprecated, not
  removed**: the API and the config file accept and return it, the snapshot
  still carries `McpOrgSettingsConfig::fail_open` at format 1, and the gateway
  ignores it; the dashboard shows it as having no effect. The snapshot always
  carries `fail_open = false` (`load_mcp_settings` no longer reads the column),
  so a gateway that predates the change refuses an unreadable body too during
  a rolling upgrade, while the API and the export keep the stored value. Removal is tracked for
  2.0.0. It never had a say over an upstream that fails. The guardrail-provider
  and plugin `failure_mode` (an unreachable engine) is a different setting and is
  unchanged.
- **Transport.** The server row is authoritative at request time; the org
  `default_transport` is applied by `create_server` when the request names none.

## Endpoints

- `GET`/`POST /api/v1/orgs/{org_id}/mcp-servers`, `PATCH`/`DELETE /api/v1/mcp-servers/{id}` — viewer reads, admin writes. A server URL must be `http(s)`; the transport must be one of `stdio`, `sse`, `streamable_http`, `websocket`. `PATCH` updates registry metadata, enabled state, declared tools and required scopes without destroying grants or sessions.
- `POST /api/v1/mcp-servers/{id}/discover`, `GET /api/v1/mcp-servers/{id}/tools` — ask the server for its tools now (admin), and read the manifest with schemas and stale flags (viewer). See [Tool discovery](#tool-discovery).
- `POST /api/v1/mcp-servers/{id}/oauth/authorize` — body `{scopes?, step_up_scopes?}`; see [Scope selection](#scope-selection-and-insufficient_scope-step-up-1417).
- `GET /api/v1/orgs/{org_id}/mcp/library` — curated definitions annotated with whether the slug is installed. Installing one uses the ordinary server create endpoint, so the registry remains the source of truth. See [The curated catalog](#the-curated-catalog) for what an entry declares.
- `GET`/`POST /api/v1/orgs/{org_id}/mcp/tool-groups`, `PUT`/`DELETE /api/v1/mcp/tool-groups/{id}` — exact server/tool policy manifests, enforced by the proxy as an access boundary (see [Tool groups](#tool-groups-as-an-access-boundary)).
- `GET`/`PUT /api/v1/orgs/{org_id}/mcp/settings` — organization transport and request defaults. The current HTTP proxy still uses deployment-level transport timeouts, which is why `mcp_settings` carries the `experimental` [stability marker](../development/stability-markers.md).
- `GET /api/v1/orgs/{org_id}/mcp/grants`, `DELETE /api/v1/mcp/grants/{id}`
- `GET /api/v1/orgs/{org_id}/mcp/sessions`, `DELETE /api/v1/mcp/sessions/{id}`
- `GET`/`PUT /api/v1/mcp-servers/{id}/oauth-client` — the OAuth client rolter presents to the server's authorization server. Admin-only in both directions: the row names a third party the tenant has chosen to trust. Only `client_id` is required: `authorize_url` and `token_url` are the fallback for a server that publishes no metadata and must be sent together or not at all, `issuer` pins the authorization server's issuer identifier for RFC 9207 validation, and `discovery` is `auto` (the default) or `manual`. The client secret is sealed with the deployment KEK on write and never read back; `PUT` with an empty secret downgrades a confidential client to a public one. The response also reports the RFC 8707 `resource` every request for this server will carry, and what the last discovery resolved.
- `POST /api/v1/mcp-servers/{id}/oauth/authorize` — begin consent. Returns the authorization URL rather than a `302`, because the caller is the dashboard over `fetch` and cannot usefully follow a cross-origin redirect. The grant's owner is `Principal::account_id()` (#2859), so a signed-in superadmin consents as themselves: matching `Principal::User` alone turned them away, because `Principal::for_user` makes a superadmin session `Principal::Superadmin { account: Some(id) }`. The admin token and open mode carry `account: None` and are refused with a `400` naming both, since a grant owned by nobody could not be listed or revoked by anyone.
- `GET /auth/mcp/callback` — where the browser returns. Authenticated by the one-shot login state, not by a session bearer token. A browser is redirected on to the dashboard and any other caller gets JSON; see [How the callback answers](#how-the-callback-answers).
- `POST /api/v1/mcp/sessions/{id}/refresh` — renew a session from its stored refresh token. Failures answer by whose fault they are (#2919): the authorization server refusing the grant is a `400` and revokes the session; the authorization server being unreachable, timing out or answering `5xx` or junk is a `502` with code `authorization_server_unavailable` and one fixed sentence, leaving the session alone; a fault in rolter's own store or sealing is the generic `500`; a missing `ROLTER_KEK` stays a `400` naming the setting. The token endpoint's address and the store's words go to the control-plane log, never to the body. `POST .../exchange` and the JSON answer of `GET /auth/mcp/callback` map their token failures the same way, through `TokenError::into_api`.
- `POST /api/v1/mcp/sessions/{id}/exchange` — RFC 8693 token exchange for a narrower, downstream session.
- `GET`/`POST`/`DELETE /mcp/{server_slug}/{path...}` — Streamable HTTP/SSE proxy on the gateway, authorized by virtual-key owner, server and required scopes

Revoking a grant revokes every session under it **in the same transaction**, so consent and tokens can never disagree. Server creation/deletion and both revocations are written to `audit_log`.

## Which specification revision this targets

The flow implements the MCP authorization specification revision **`draft`**, as
published at `modelcontextprotocol.io/specification/draft/basic/authorization`
and read on **2026-09-08** (#1347). That date is the thing to check first when
the next drift is suspected: the specification moves, and #707 shipped a client
that was correct against the revision of its day and had gone three MUSTs stale
by the time #1347 was filed.

Three client-side requirements are load-bearing, and all three fail closed — a
check that does not pass ends the flow rather than logging and continuing.

### RFC 9728 — discovery of the authorization server

An MCP server publishes protected resource metadata naming its authorization
servers, and a client is required to use it rather than to be told where to go.
`crates/rolter-control/src/mcp_oauth_discovery.rs` walks the specification's
order:

1. an unauthenticated `GET` of the server URL, reading `resource_metadata` out
   of the `WWW-Authenticate` challenge on a `401`;
2. `/.well-known/oauth-protected-resource` with the server's path inserted
   **after** the suffix (`https://h/.well-known/oauth-protected-resource/mcp`,
   not `https://h/mcp/.well-known/…` — RFC 9728 §3.1 puts it the unusual way
   round);
3. the same document at the root.

The document must declare the resource it was fetched for (RFC 9728 §3.3), or a
server could hand rolter somebody else's authorization server. Each listed
authorization server is then probed for RFC 8414 or OpenID Connect metadata in
the required priority order, and the document's `issuer` must be identical to
the identifier the URL was built from (RFC 8414 §3.3).

Only the interactive `POST .../oauth/authorize` probes. What it resolves is
cached on the `mcp_servers` row, and the background refresher and the token
exchange read that cache, so nothing off a user's request reaches out to an
upstream. The cache write is skipped when the values have not changed:
`mcp_servers` carries a statement-level `bump_config_version()` trigger, and an
unconditional write would wake every gateway on every consent.

**Moving a server's URL invalidates the cache** (#1416). The cached endpoints
belong to whatever authorization server the _old_ URL's metadata named, so
`PATCH /api/v1/mcp-servers/{id}` clears all four `oauth_discovered_*` columns
whenever the update actually changes `url`. Without that, a refresh landing
between the edit and the next interactive authorize would post to the previous
server's token endpoint while naming the new canonical URI in `resource` — it
fails closed, since a compliant authorization server refuses the mismatched
audience, but it fails for the wrong reason and re-discovery is the right
answer. The clearing is part of the same `update` statement that writes the URL,
for the trigger reason above: a second statement would bump `config_version`
twice for one edit, and an edit that leaves `url` alone clears nothing.

**Hand-configured endpoints remain the fallback.** `authorize_url` and
`token_url` are now optional on `PUT .../oauth-client`, and the preference order
is discovery, then the last cached discovery, then what an operator typed. A row
configured before #1347 keeps working untouched: discovery is attempted, finds
nothing for a server that publishes nothing, and the configured pair is used.
`"discovery": "manual"` pins a server to the configured pair and skips the probe
entirely, which is worth setting for a server known to publish no metadata.

**Re-pinning the issuer or the discovery mode invalidates the cache the same
way** (#1432). The cached triple belongs to whatever authorization server the
_old_ issuer and discovery choice named, so `PUT .../oauth-client` clears all
four `oauth_discovered_*` columns whenever the update actually changes
`oauth_issuer` or `oauth_discovery` — same trigger, same reasoning as the URL
case above, and folded into the same statement for the same
`bump_config_version()` reason. Without it, an operator correcting a rotated
`oauth_issuer` while staying on `"discovery": "auto"` saw no effect on the
refresher or the on-behalf-of exchange, which never probe and were still
reading the stale cache; only the next interactive `POST .../oauth/authorize`
happened to re-run discovery and overwrite it. An edit that leaves both fields
alone — a secret rotation, say — clears nothing.

### RFC 8707 — resource indicators

Every authorization request and every token request — code, refresh and exchange
alike — carries `resource`, the canonical URI of the MCP server the token is
for. Without it the token is not audience-bound, and a compliant server, which
MUST reject a token that was not issued for it, is entitled to refuse every
token rolter mints.

The canonical form is computed in exactly one place, `ResourceUri::parse`:

- scheme and host lowercased (uppercase is accepted on the way in);
- **no fragment** — RFC 8707 §2 forbids one outright;
- **no trailing slash**, the form the specification asks implementations to
  settle on;
- the query preserved, since RFC 8707 allows one where it is what scopes the
  resource;
- userinfo refused, since a credential has no business in an authorization URL.

The two requests cannot disagree about it, and that is structural rather than a
convention: `ResourceUri` is the only way to name a resource in the crate, its
only constructor is that parser, and both `authorization_url()` and
`post_token()` take one. The value used in the authorization request is recorded
on the login state and re-parsed at the callback — parsing is idempotent, so the
token request carries the identical string. `post_token()` then refuses outright
to send a form with no `resource`, which is the guard that catches a future
refactor rather than a present bug.

### RFC 9207 — issuer validation

Before the browser leaves, the issuer of the validated authorization-server
metadata is recorded on `mcp_oauth_login_states` beside the sealed PKCE
verifier, together with whether that metadata advertised
`authorization_response_iss_parameter_supported`. The callback applies the
RFC 9207 §2.4 table before the authorization code goes anywhere:

| advertised        | `iss` present | action                              |
| ----------------- | ------------- | ----------------------------------- |
| `true`            | yes           | compare against the recorded issuer |
| `true`            | no            | **reject**                          |
| `false` or absent | yes           | compare against the recorded issuer |
| `false` or absent | no            | proceed                             |

The comparison is **byte equality**. No case folding, no default-port elision,
no trailing slash, no percent-decoding — reaching for a URL parser here would
re-introduce exactly the equivalences a mix-up attack needs. A response that
fails the check is refused whole: its `error`, `error_description` and
`error_uri` are not acted on or displayed either, which is why the callback
resolves the login state and validates the issuer _before_ it reads anything
else in the response.

Which line applies is visible over the API: `GET /api/v1/mcp-servers/{id}/oauth-client`
returns `discovered_iss_supported` beside the rest of the discovery cache
(#1569). Without it a client reading only that endpoint can see _what_ was
discovered but not how a callback with no `iss` will be judged, which is the
difference between "proceed" and "reject". Note the naming: the row and the
server list use `oauth_discovered_*`, the client view drops the prefix.

A row with no recorded issuer at all — hand-configured, no metadata, no pinned
`issuer` — lands on the last line of the table and keeps working exactly as it
did. But an `iss` that arrives for such a row is **refused**, because there is
nothing authentic to compare it against and accepting it would be validation in
name only. The fix an operator is told about is to pin `issuer` on the OAuth
client, or to let discovery resolve one.

### Scope selection and `insufficient_scope` step-up (#1417)

The three MUSTs above are not the whole of the specification's scope guidance.
The SHOULD requirements are implemented too, and are not fail-closed: they choose what to
ask for, and a resource that publishes nothing leaves the operator's
configuration in charge.

**Which scopes an authorization request asks for**, highest precedence first
(`select_scopes` in `mcp_oauth_flow.rs`, recorded as `scope_source` in the
`mcp_oauth_grant.authorize` audit row):

1. `scopes` in the request body, used exactly as given (`explicit`).
2. `scope` from the `WWW-Authenticate` challenge of an unauthenticated request
   to the resource, plus the server's `required_scopes` (`challenge`). This is
   what the resource itself says the request needed, and it outranks whatever
   the operator configured.
3. The server's `default_scopes` (`configured`).
4. `scopes_supported` from the protected resource metadata, plus
   `required_scopes` (`supported`). It is the resource's whole vocabulary, not
   a statement of need, so it only answers when nothing more specific does.
5. `required_scopes` alone (`required`), which can be empty.

The challenge and metadata are read at authorize time only, by the person who
is waiting; they are not cached on the row and the refresher never probes for
them. Every scope token from either source is validated (RFC 6749 §3.3
charset, at most 128 bytes, at most 32 tokens), so a hostile resource cannot
smuggle a quote or a space into the authorization URL. Challenge parsing lives
in `rolter_core::mcp_challenge`, shared with the gateway.

**Step-up.** When an upstream answers a proxied call `403` with
`WWW-Authenticate: Bearer error="insufficient_scope"`, the gateway, for an
`oauth` server only, replies `403` with the code `mcp_insufficient_scope`, the
scopes it named (validated) in `error.required_scopes`, and a rebuilt
`WWW-Authenticate` challenge without the upstream's `resource_metadata`. The
call is logged as `auth_denied`. The gateway cannot consent on anyone's
behalf, so it does not try: the owner of the key re-consents with
`POST /api/v1/mcp-servers/{id}/oauth/authorize` and `step_up_scopes`, which asks
for the live grant's scopes, the new ones and the server's `required_scopes`
together so the re-consent widens the grant rather than replacing it. Until they
do, every such call fails the same way, which is also what an operator who is
not present sees: no scope is ever added without a person consenting to it. A
server with a static credential has no consent to repeat, so its `403` is
relayed as the upstream sent it.

The dashboard half (#3296) is `ui/src/components/McpStepUp.tsx` and `ui/src/lib/mcp-step-up.ts`. `mcp_insufficient_scope` is an answer to the gateway's caller, and the log row of that refusal records the scopes the upstream asked for in `error_scopes` (#3375), so the detail of an `auth_denied` row offers exactly those the caller's longest-lived live session does not hold. A row without them (written before ClickHouse migration `042`, or refused for another reason) falls back to the server's declared `required_scopes` that the session lacks (the check the gateway makes before forwarding), and lets the key's owner name a scope when nothing declared is missing. The server card offers the declared ones. The action is `startMcpStepUp`, the same pop-up flow as Connect (`useMcpConnect`) carrying `step_up_scopes`.

### Fetching from the control plane, and SSRF

Discovery means the control plane now fetches URLs it did not fetch before: one
derived from the operator-configured server URL, and — the new exposure — the
`authorization_servers` and endpoints of a document a third-party MCP server
served. An operator registering an MCP server is already trusted to point the
control plane at a token endpoint, so this widens an existing trust rather than
creating one, but it widens it to a value the _upstream_ chooses. Three guards
bound it, and they are the reason this is acceptable rather than merely small:

- every URL is `https`, with `http` allowed only on loopback;
- every URL goes through the deployment's egress policy, which denies
  link-local (instance metadata) by default and private and loopback ranges when
  configured to;
- discovery uses its own HTTP client with **redirects disabled**, so a host that
  passed the egress check cannot hand the request to one that would not have,
  and with a 5-second timeout and a 64 KiB body cap.

What remains uncovered is what the egress policy itself does not cover: a
hostname that resolves to a denied address, since the policy classifies IP
literals and does not resolve DNS. That is the same connect-time gap every other
upstream in rolter has, and it is not made worse here.

## The session lifecycle

Consent runs as an ordinary authorization-code flow with PKCE. `POST .../oauth/authorize` mints a verifier, seals it into a one-shot `mcp_oauth_login_states` row keyed by `state`, and returns the authorization URL. The callback consumes that row — a replayed `state` finds nothing and fails — verifies the code against the sealed verifier, then writes the grant and its first session in one transaction.

A background refresher sweeps every 60 seconds and renews up to 100 sessions per pass, 5 minutes before expiry, so the skew between rolter's clock and the authorization server's plus one round trip is always covered. It handles refresh-token rotation by replacing the stored refresh material whenever the response carries a new one. **A permanently refused refresh revokes the session** rather than retrying: a `4xx` carrying `invalid_grant` or `invalid_scope` is a final answer about this grant — consent was withdrawn or the token was rotated away — and a retry loop would only hammer the upstream. Transient failures (network errors, `5xx`) leave the session alone for the next sweep, and so do rolter's own store and sealing faults (`TokenError::Store`): neither is a verdict on the grant. The sweeper logs the cause of every such failure; a transient one has had its token-endpoint address masked when it was raised, so the log is no more revealing than the `502` an interactive caller sees.

Token exchange (`urn:ietf:params:oauth:grant-type:token-exchange`) is the server-to-server half. A service acting for a user gets its own session row descending from the same grant, so it can be revoked independently without taking the user's interactive session with it, and its scopes are intersected against the grant's — an exchange can never widen consent.

### How the callback answers

The callback negotiates on `Accept` (#2166). A request that accepts `text/html` is a browser: the authorization server sending the user back. It gets a `303 See Other` to the dashboard's Auth Sessions screen, built on the deployment's public URL like the redirect URI itself, with the outcome in the query string:

- `consent=completed&session=<id>&server=<id>` when the grant and its first session were written;
- `consent=failed&reason=<code>` when anything refused, plus `&server=<id>` once the login state has named the server.

Every other caller, `reqwest`, `curl` and `fetch` included, gets what it always did: `ConsentCompleted` as JSON, or the usual error body. The response carries `Vary: Accept` either way, and `GET /openapi.json` documents both answers.

Only identifiers the caller could already list go in that URL. The authorization code, the `state`, the tokens and anything the authorization server said stay out: a failure travels as its family, and the dashboard supplies the words. That is also what RFC 9207 asks of a response whose issuer did not check out, which may not be displayed at all. The codes are `ConsentFailure` in `crates/rolter-control/src/mcp_oauth_flow.rs`, and they are published values, so one is never renamed:

| `reason`                | when                                                                           |
| ----------------------- | ------------------------------------------------------------------------------ |
| `access_denied`         | the authorization server answered `error=access_denied`                        |
| `authorization_failed`  | it answered with any other `error`                                             |
| `state_invalid`         | no `state`, no `code`, or a state that is unknown, expired or already redeemed |
| `issuer_mismatch`       | the RFC 9207 check failed and the code was discarded                           |
| `token_exchange_failed` | the token endpoint refused the code or could not be reached                    |
| `not_configured`        | no `ROLTER_KEK`, or no resource or OAuth client resolves for the server        |
| `internal_error`        | anything else, a database error most likely                                    |

A browser learns only the family, so the handler logs every failure at `info` with the code, the server and the full error, which is where an operator finds the detail. `ui/src/lib/mcp-consent.test.ts` reads the Rust enum and fails when the dashboard's list of codes drifts from it; a code the dashboard does not know still renders a generic sentence.

## In the dashboard

The whole flow is drivable from the SPA (#1194), so an operator never has to reach for `curl` to register a client:

- **MCP Catalog → Configure** carries an _Authentication_ section (#1447) that shows only what the selected `auth_kind` uses: nothing for `none`, a write-only credential for `bearer`, a header name plus credential for `header`, and the OAuth client section below for `oauth`. It writes through `PUT .../auth` after the row is saved, and only when the kind, the header name or a typed credential actually moved. That keeps untouched saves out of `audit_log`, and it keeps a deployment without `ROLTER_KEK` able to edit a server's other fields, because the route needs the KEK even for a kind that seals nothing. The header name is checked against the same reserved list as `RESERVED_AUTH_HEADERS` before sending. _Clear stored credential_ is a confirmed, immediate move to `none`, because the shape constraint allows no `bearer`/`header` row without a credential. Saving a credentialed server as `none` or `oauth` asks for the same confirmation. A `ROLTER_KEK` refusal is shown as a deployment problem next to the control plane's own message. Register server and Configure open one sheet in four sections (Endpoint, Tools, Authentication, Transport overrides; the last starts folded unless the server sets an override), and the OAuth fields exist only under the `oauth` method (#2170). The sheet also edits the three transport overrides: a blank field is `null`, and a field left as it was is omitted from the `PATCH`, so absent and `null` stay distinct on the wire. The rules live in `ui/src/lib/mcp-server-auth.ts` with unit tests.
- The _OAuth client_ section (the `oauth` branch above) has: a discovery-mode picker (`auto` | `manual`), client id, a write-only client secret, the authorize/token URL pair, a pinned `issuer` and the default scopes (#1415). Only the client id is required under `auto`, where the pair is an optional fallback; `manual` requires the pair, and half a pair is refused in either mode. `discovery` and `issuer` are sent on every `PUT` — the handler reads an omitted mode as `auto`, which used to reset a `manual` server on every re-save. Under `auto` a read-only panel shows the row's `oauth_discovered_*` columns with `oauth_discovered_at`, or says nothing has been discovered yet, and warns that saving a new server URL, issuer or mode clears it — the same three changes the store clears the cache on. A moved URL is also flagged beside the URL field whatever the auth kind, since the store drops the cache on a URL change even when the panel is not on screen (#1572). _Connect_ is enabled for a client id alone under `auto`, and needs the pair under `manual`. The rules live in `ui/src/lib/mcp-oauth-client.ts` with unit tests. It reads `GET .../oauth-client` purely for `redirect_uri` — the callback is deployment-derived and cannot be worked out from the browser's origin — and writes through `PUT .../oauth-client` after the server row itself is saved, which is also how a client is registered on a server in the same action that creates it. The secret is never echoed back: a badge says whether one is stored, and a _Clear stored secret_ toggle sends `""` to downgrade the client to a public one. The `PUT` is skipped entirely when nothing in the section changed, so re-saving a server does not fill `audit_log` with `mcp_oauth_client.update` entries nobody made.
- **MCP Catalog → Connect** calls `POST .../oauth/authorize` and opens the returned URL in a new tab. The dashboard never navigates itself there: the consent screen belongs to a third party, and a blocked pop-up is reported rather than left silent, because the request has already succeeded by then.
- **Where consent ends** (#2166). The new tab comes back through the callback and lands on **Auth Sessions**, which reads the outcome from its query string (`ui/src/lib/mcp-consent.ts`) and states it above the listing, before the listing has loaded: the server by name and the new session highlighted, or the failure in the dashboard's own words for each `reason`, with a link back to the catalog. On success it posts one message on the `rolter.mcp-consent` `BroadcastChannel`. The catalog tab that started the flow listens and invalidates its `mcp-grants` and `mcp-sessions` queries, so the card marks **Connected** without a reload; a browser without `BroadcastChannel` falls back to the refetch a query does when its window regains focus. The mark means the signed-in account holds an active grant with an unrevoked, unexpired session under it. It is filtered to that account on purpose, since an admin's listings carry every member's rows.
- **Auth Sessions → renew** calls `POST /api/v1/mcp/sessions/{id}/refresh` for one row, beside the background sweeper. Only a session that stored a refresh token offers it. A refusal is worth reading rather than retrying, since the control plane has already revoked the session by the time the error arrives.

## Static credentials, beside OAuth

OAuth is one of four things `mcp_servers.auth_kind` can name (#952). The others
are `none`, `bearer` and `header` — one deployment-wide credential rather than a
token minted per user — and they exist because most hosted MCP servers today
issue a long-lived token or an API key rather than running an authorization
server.

The schema, not the handler, is what keeps the four coherent. `mcp_servers_auth_kind_shape`
requires `bearer` and `header` to hold a sealed credential and forbids one to
`none` and `oauth`, and requires a header name for `header` alone. So a row
cannot claim to be unauthenticated while holding a secret, and `has_credential`
in the read API cannot contradict what the row says it does. The handler checks
the same rules first only so an operator gets a `400` naming the problem instead
of a `500` from the constraint.

`mcp_servers_auth_header_name_shape` restricts the header to an RFC 9110 field
name and refuses ten reserved ones. `Authorization` is the one that matters:
without that clause, header mode could present an API key exactly where the
bearer path puts a token.

The credential is sealed with the deployment KEK through the same
[`Kek`](../deployment/backup-and-restore.md) helper as the OAuth client secret,
registered in `SEALED_COLUMNS`, and projected as `has_credential` — the
plaintext has no field on any `Serialize` type. Writes follow the same
three-shape convention the client secret uses: absent leaves the stored value,
`""` clears it, anything else replaces it. That is what lets an operator move a
server from `bearer` to `header` without re-typing a secret the API gives them
no way to read.

Moving to a kind that carries no credential clears it rather than orphaning it,
whatever the caller sent. A sealed secret nothing can use is one `rolter kek
verify` would audit forever, and it is a credential still sitting in a backup
for no reason.

A stored credential reaches the data plane through `/internal/snapshot` (#952).
`load_mcp_servers` unseals it with the KEK and puts the plaintext on
`McpServerConfig`, the same route an OAuth access token already takes — that
endpoint is the credential-bearing control-to-data-plane channel, so this adds a
secret to it rather than opening a new path for one. `McpServerConfig`'s `Debug`
redacts the field so formatting a snapshot cannot spill it into a log or a crash
dump, and the header form marks the value sensitive before it reaches the wire.

A row whose credential cannot be unsealed — `ROLTER_KEK` unset or rotated —
keeps its other fields and loses only the secret, and the proxy then refuses
that server with `mcp_credential_unavailable` rather than calling it
unauthenticated. Dropping the server from the snapshot instead would make a
misconfigured KEK look like a server that was never registered.

### Per-server transport overrides

`connect_timeout_ms`, `request_timeout_ms` and `max_retries` are nullable on
`mcp_servers`, and the proxy reads them (#952). Null means inherit rather than a
copy taken at creation, so the value a server never overrode still moves when
the value it inherits does.

What it inherits is the _deployment_ transport timeout, not the org's
`mcp_gateway_settings`: those org defaults are still not read by the data plane,
which is what the `mcp_settings` stability marker records and what #1404 closes.
Until then a per-server override is the only way to give one server its own
budget.

reqwest bakes the connect timeout into the client rather than the request, so a
per-server `connect_timeout_ms` means a client per distinct value; they are
built once and cached, and there are only ever a handful. `max_retries` repeats
only a send that failed to connect: an MCP call is frequently a tool invocation
and is not idempotent, so a request the server may have seen — including one
that timed out — is reported rather than replayed.

On the wire the `PATCH` distinguishes absent from null — leave the override
versus drop it — which an `Option` alone cannot express. serde collapses both to
`None` for an `Option<Option<T>>`, so the fields deserialize through
`explicit_null`, which returns `Some(None)` for a present null. Without it
"stop overriding" silently means "leave it".

## The tool-call log

The gateway records every proxied JSON-RPC `tools/call` in ClickHouse's
`mcp_tool_call_logs` (#2395). Before this nothing did: the control plane's
`POST /api/v1/mcp/events` had no caller, so Observability → MCP Logs was empty.

- **Hook.** `mcp_proxy::proxy` reads a `POST` body **once** (`mcp_policy::Read`,
  #3399) once the server and key are resolved and before any credential is
  chosen, so the refusals below are recorded too. The tool policy judges that
  parse and the log takes its `PendingCall` from it, so both look keys up the
  way a Go decoder does (case-folded): a `tools/call` written `Method` or
  `Params` that the policy allows is recorded under the tool it was judged as.
  Every allowed `tools/call` is recorded (#3466): a single request, each call of
  a JSON-RPC batch and a notification; `initialize`, `tools/list` and the other
  methods are not. The calls are moved out of the parsed body
  (`Read::into_calls`) only once they are needed, by a refusal before the
  request is forwarded or by the forward itself, so a request refused for
  another reason pays nothing for the log. Nothing in the body is cloned: the
  policy asks the tool a call names before it copies anything of it, so an
  allowed call costs no clone, and only a refused call is copied into the
  canonical request its row is made from. The reply is parsed once (`Replies`,
  indexed by the id's JSON text) and each call takes the reply with its own
  `id`, whatever order a batch's replies come in; two calls sharing an id get one
  reply each.
  A request is bounded only by `max_body_bytes`, and every pending row is held
  through the wait on the upstream, so at most `MAX_CALLS_PER_REQUEST` (256)
  calls of one request become rows, allowed or refused. The rest are still
  judged and forwarded or refused, and are counted in
  `rolter_mcp_events_dropped_total`.
  A batch the policy refuses is refused whole and forwarded nowhere, so only its
  refused calls are recorded (`auth_denied`); an allowed call beside them never
  reached the server and has no row. A batch whose reply is rewritten (it holds a
  `tools/list`, so the reply is narrowed) cannot be read for its calls, which are
  recorded with the HTTP layer's status and no result. A refused call is recorded
  whatever its shape. The
  body is not parsed at all unless a rule applies to the caller or the sink is
  enabled and the bytes hold `tools/call` or an escape that could spell it; with
  no `clickhouse_url` the sink is disabled and nothing is parsed for the log.
- **Insufficient scope (#3375).** A call an OAuth upstream refuses with
  `insufficient_scope` is recorded `auth_denied` with the error
  `insufficient scope` (`INSUFFICIENT_SCOPE_ERROR`) and the challenge's
  validated scopes in `error_scopes` (`clickhouse/042_mcp_log_error_scopes.sql`,
  `Array(String)`), the same list the caller gets as `error.required_scopes`.
  The field is left out of every other row and the writer sends
  `input_format_skip_unknown_fields=1`, so an un-migrated ClickHouse keeps
  accepting rows. The control plane's detail read probes `system.columns` for
  the column (`McpColumnCache`, the lifetimes of the request-log probe) and
  selects an empty array in its place until the migration is applied.
- **Notifications (#3466).** A `tools/call` without an `id` (or with a `null`
  one, which cannot be matched to a reply) has no reply to read its outcome from. It
  is recorded when the upstream's response arrives, with the status the HTTP
  layer gave (`success` for a `2xx`, `auth_denied` for `401`/`403`, and so on) and
  `notification = 1` (`clickhouse/047_mcp_log_notification.sql`, `UInt8`), which
  says that status is not what the tool did. The field is left out of every other
  row and the writer sends `input_format_skip_unknown_fields=1`, so an
  un-migrated ClickHouse keeps accepting rows and records a notification as an
  ordinary `success` row. A notification the policy refuses is a refusal like
  any other and carries no flag. The control plane's detail read probes for the
  column with the one that probes `error_scopes` (`McpColumns`) and selects
  `0` in its place until the migration is applied; `POST /api/v1/mcp/events`
  accepts `notification` and stores it only when true.
- **Write path.** `McpEventSink` (`crates/rolter-gateway/src/mcp_log.rs`) is a
  bounded channel and a batch writer, built like the request-log and
  health-event writers and sharing their `[logging]` `batch_max`, `flush_ms`
  and `queue_capacity`. `emit` is a `try_send`: a full queue drops the event and
  counts it in `rolter_mcp_events_dropped_total` (`..._written_total` counts
  rows that landed), and a call is never delayed or failed by the log. The
  writer is stopped through `SinkTasks` by `AppState::drain_sinks`, so queued
  events survive a `SIGTERM`.
- **Attribution.** `org_id`, `team_id`, `project_id` and `virtual_key_id` are the
  calling virtual key's. `user_id` is the key's owner, which on an `oauth`
  server is the OAuth session owner: the proxy selects the session by
  `(server, key owner)`, so the two are one id by construction. The control
  plane scopes MCP log reads by these columns.
- **Status** is the closed set the ingest route validates. A JSON-RPC `error`
  reply or a `result` with `isError: true` is `error`; a refused session
  (missing, expired, scope not covered) or an upstream `401`/`403` is
  `auth_denied`; a forwarder timeout, `408` or `504` is `timeout`; an
  unreachable or failing upstream (`5xx`, broken body) is `transport_error`; any
  other upstream `4xx` is `error`. The `error` column holds a fixed category,
  never upstream text.
- **Capture.** `arguments` and `result` use `rolter_core::mcp_log::capture`, the
  same helper the ingest route calls: empty unless `[logging.payload_capture]`
  is `enabled`, redacted with the `PayloadRedactor` compiled from the same
  policy (`redact_fields`, the built-in detectors and the operator patterns,
  #3277) and then truncated to `max_bytes`. The gateway takes the redactor
  from the snapshot (`Snapshot::payload_redactor`); the ingest route compiles
  it from the stored policy. A policy that does not compile leaves the key rule
  in force.
  The `models` and `virtual_key_ids` allow-lists apply to LLM payloads only, as
  they do on the ingest route. `result` holds the JSON-RPC `result`, or the
  `error` object for an error reply.
- **Streaming.** The response body is passed through untouched and a bounded
  copy (1 MiB) is kept. The call is recorded when the body ends, or when the
  client hangs up, and `latency_ms` is the time to that point, not to the first
  byte. For an SSE body the reply is the `data:` event whose `id` matches the
  request, among any progress notifications. A body over 1 MiB is recorded with
  the status the HTTP layer gave and no `result`; a client that disconnects
  before the reply arrived is a `transport_error`.

### Not supported, and why

- **stdio** — removed as a transport in #783, since a hosted control plane
  cannot dial a local subprocess. The MCP specification agrees from the other
  side: stdio implementations _"SHOULD NOT"_ use its authorization flow and
  should _"retrieve credentials from the environment"_, so there is nothing here
  to store for one.
- **mTLS** — a client certificate is an identity, not a string, and there is no
  certificate store to put one in.
- **Dynamic Client Registration (RFC 7591)** — the specification now marks it
  deprecated, retained only for authorization servers that do not support Client
  ID Metadata Documents. The decision recorded in #1347 is that rolter does not
  implement it and will not: if client registration is ever automated here it
  will be **Client ID Metadata Documents**
  (`draft-ietf-oauth-client-id-metadata-document-00`), which the specification
  now says clients SHOULD support, and DCR would be a second registration path
  to keep secure for a mechanism its own specification is walking away from. A
  `client_id` therefore stays operator-supplied; it is the one part of the
  client the flow will not discover.

`header` mode is deliberately outside the specification, which defines only
`Authorization: Bearer`. It is an accommodation for real servers, and both the
operator docs and this page say so rather than implying conformance.
