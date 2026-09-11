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
- **Grant** — a user's consent against a server, with the scope set they agreed to. A user holds at most one *live* grant per server (a partial unique index on `revoked_at is null`); revoked grants are kept so the audit trail survives. Re-consenting updates the scopes in place rather than accumulating rows.
- **Session** — the tokens issued under a grant, with `expires_at`, an optional refresh token and `refresh_expires_at`.

## Token handling

Access and refresh tokens are sealed with AES-256-GCM under the deployment KEK (`ROLTER_KEK`), the same mechanism as upstream provider credentials — there is deliberately no plaintext column for either. Ciphertext and nonce sit side by side; the KEK never reaches the database.

The store exposes tokens through two credential-only paths. `McpOAuthRepo::open_session` opens one explicitly selected live session for lifecycle code. The Postgres config store opens only the newest live session per `(server, user)` whose scopes remain within its live grant and cover the server's required scopes; those records travel through the token-guarded `/internal/snapshot` channel already used for decrypted provider credentials. Public API DTOs carry only metadata and a `has_refresh_token` boolean.

The gateway indexes servers by `(org, slug)` and sessions by `(server, user)`. A request to `/mcp/{server}` must authenticate with a database-backed virtual key whose `created_by` user owns the selected session. The gateway repeats the required-scope check before connecting and replaces the caller's virtual key with the downstream bearer token. Revocation, expiry and policy writes bump `config_version`, so snapshot polling removes authorization without a restart.

## Who sees what

| caller | grants / sessions visible | may revoke |
| --- | --- | --- |
| superadmin / admin token | every one in the org | any |
| org admin | every one in the org | any |
| org member or viewer | only the ones they own | only their own |
| anyone outside the org | none (`403`) | none (`403`) |

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

- **The lists are a hand-taken snapshot, not discovery.** Nothing re-checks them
  against the live server, so a tool renamed upstream is stale here until
  someone edits the file. Calling `tools/list` on the server after install is
  the real fix and is tracked separately; this is the cheap step that stops the
  catalog from asserting something false.
- **An empty `required_scopes` can be the accurate answer.** Notion's remote
  server grants access to the pages the user picks on its consent screen rather
  than to named scopes, so it asks for none. Every other entry declaring an
  empty list would be an unfilled row, and a test enforces that distinction.

## Endpoints

- `GET`/`POST /api/v1/orgs/{org_id}/mcp-servers`, `PATCH`/`DELETE /api/v1/mcp-servers/{id}` — viewer reads, admin writes. A server URL must be `http(s)`; the transport must be one of `stdio`, `sse`, `streamable_http`, `websocket`. `PATCH` updates registry metadata, enabled state, declared tools and required scopes without destroying grants or sessions.
- `GET /api/v1/orgs/{org_id}/mcp/library` — curated definitions annotated with whether the slug is installed. Installing one uses the ordinary server create endpoint, so the registry remains the source of truth. See [The curated catalog](#the-curated-catalog) for what an entry declares.
- `GET`/`POST /api/v1/orgs/{org_id}/mcp/tool-groups`, `PUT`/`DELETE /api/v1/mcp/tool-groups/{id}` — exact server/tool policy manifests. These definitions are not yet enforced by the proxy, which is why `mcp_tool_groups` carries the `experimental` [stability marker](../development/stability-markers.md).
- `GET`/`PUT /api/v1/orgs/{org_id}/mcp/settings` — organization transport and request defaults. The current HTTP proxy still uses deployment-level transport timeouts, which is why `mcp_settings` carries the `experimental` [stability marker](../development/stability-markers.md).
- `GET /api/v1/orgs/{org_id}/mcp/grants`, `DELETE /api/v1/mcp/grants/{id}`
- `GET /api/v1/orgs/{org_id}/mcp/sessions`, `DELETE /api/v1/mcp/sessions/{id}`
- `GET`/`PUT /api/v1/mcp-servers/{id}/oauth-client` — the OAuth client rolter presents to the server's authorization server. Admin-only in both directions: the row names a third party the tenant has chosen to trust. Only `client_id` is required: `authorize_url` and `token_url` are the fallback for a server that publishes no metadata and must be sent together or not at all, `issuer` pins the authorization server's issuer identifier for RFC 9207 validation, and `discovery` is `auto` (the default) or `manual`. The client secret is sealed with the deployment KEK on write and never read back; `PUT` with an empty secret downgrades a confidential client to a public one. The response also reports the RFC 8707 `resource` every request for this server will carry, and what the last discovery resolved.
- `POST /api/v1/mcp-servers/{id}/oauth/authorize` — begin consent. Returns the authorization URL rather than a `302`, because the caller is the dashboard over `fetch` and cannot usefully follow a cross-origin redirect.
- `GET /auth/mcp/callback` — where the browser returns. Authenticated by the one-shot login state, not by a session bearer token.
- `POST /api/v1/mcp/sessions/{id}/refresh` — renew a session from its stored refresh token.
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

**Hand-configured endpoints remain the fallback.** `authorize_url` and
`token_url` are now optional on `PUT .../oauth-client`, and the preference order
is discovery, then the last cached discovery, then what an operator typed. A row
configured before #1347 keeps working untouched: discovery is attempted, finds
nothing for a server that publishes nothing, and the configured pair is used.
`"discovery": "manual"` pins a server to the configured pair and skips the probe
entirely, which is worth setting for a server known to publish no metadata.

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

| advertised | `iss` present | action |
| --- | --- | --- |
| `true` | yes | compare against the recorded issuer |
| `true` | no | **reject** |
| `false` or absent | yes | compare against the recorded issuer |
| `false` or absent | no | proceed |

The comparison is **byte equality**. No case folding, no default-port elision,
no trailing slash, no percent-decoding — reaching for a URL parser here would
re-introduce exactly the equivalences a mix-up attack needs. A response that
fails the check is refused whole: its `error`, `error_description` and
`error_uri` are not acted on or displayed either, which is why the callback
resolves the login state and validates the issuer *before* it reads anything
else in the response.

A row with no recorded issuer at all — hand-configured, no metadata, no pinned
`issuer` — lands on the last line of the table and keeps working exactly as it
did. But an `iss` that arrives for such a row is **refused**, because there is
nothing authentic to compare it against and accepting it would be validation in
name only. The fix an operator is told about is to pin `issuer` on the OAuth
client, or to let discovery resolve one.

### Fetching from the control plane, and SSRF

Discovery means the control plane now fetches URLs it did not fetch before: one
derived from the operator-configured server URL, and — the new exposure — the
`authorization_servers` and endpoints of a document a third-party MCP server
served. An operator registering an MCP server is already trusted to point the
control plane at a token endpoint, so this widens an existing trust rather than
creating one, but it widens it to a value the *upstream* chooses. Three guards
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

A background refresher sweeps every 60 seconds and renews up to 100 sessions per pass, 5 minutes before expiry, so the skew between rolter's clock and the authorization server's plus one round trip is always covered. It handles refresh-token rotation by replacing the stored refresh material whenever the response carries a new one. **A permanently refused refresh revokes the session** rather than retrying: a `4xx` carrying `invalid_grant` or `invalid_scope` is a final answer about this grant — consent was withdrawn or the token was rotated away — and a retry loop would only hammer the upstream. Transient failures (network errors, `5xx`) leave the session alone for the next sweep.

Token exchange (`urn:ietf:params:oauth:grant-type:token-exchange`) is the server-to-server half. A service acting for a user gets its own session row descending from the same grant, so it can be revoked independently without taking the user's interactive session with it, and its scopes are intersected against the grant's — an exchange can never widen consent.

## In the dashboard

The whole flow is drivable from the SPA (#1194), so an operator never has to reach for `curl` to register a client:

- **MCP Catalog → Configure** carries an *OAuth client* section: authorize URL, token URL, client id, a write-only client secret and the default scopes. It reads `GET .../oauth-client` purely for `redirect_uri` — the callback is deployment-derived and cannot be worked out from the browser's origin — and writes through `PUT .../oauth-client` after the server row itself is saved, which is also how a client is registered on a server in the same action that creates it. The secret is never echoed back: a badge says whether one is stored, and a *Clear stored secret* toggle sends `""` to downgrade the client to a public one. The `PUT` is skipped entirely when nothing in the section changed, so re-saving a server does not fill `audit_log` with `mcp_oauth_client.update` entries nobody made.
- **MCP Catalog → Connect** calls `POST .../oauth/authorize` and opens the returned URL in a new tab. The dashboard never navigates itself there: the consent screen belongs to a third party, and a blocked pop-up is reported rather than left silent, because the request has already succeeded by then.
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

All of the above is storage only. `repo::mcp::credential()` has no caller,
`McpServerConfig` carries no auth fields, so nothing crosses `/internal/snapshot`,
and `mcp_proxy` still refuses a request with no live OAuth session
(`mcp_session_unauthorized`) whatever `auth_kind` says. Spending a stored
credential on the proxy path is the remainder of #952.

### Per-server transport overrides

`connect_timeout_ms`, `request_timeout_ms` and `max_retries` are nullable on
`mcp_servers` and are meant to fall back to the org's `mcp_gateway_settings`.
Null means inherit rather than a copy taken at creation, so raising the org
default will still move every server that never asked to differ. That
resolution is not performed yet: neither the row nor `mcp_gateway_settings`
reaches the data plane, and the proxy dials on the gateway's deployment-level
transport timeouts.

On the wire the `PATCH` distinguishes absent from null — leave the override
versus drop it — which an `Option` alone cannot express. serde collapses both to
`None` for an `Option<Option<T>>`, so the fields deserialize through
`explicit_null`, which returns `Some(None)` for a present null. Without it
"stop overriding" silently means "leave it".

### Not supported, and why

- **stdio** — removed as a transport in #783, since a hosted control plane
  cannot dial a local subprocess. The MCP specification agrees from the other
  side: stdio implementations *"SHOULD NOT"* use its authorization flow and
  should *"retrieve credentials from the environment"*, so there is nothing here
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
