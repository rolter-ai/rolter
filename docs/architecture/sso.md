# Single sign-on (OIDC)

rolter can authenticate operators against an OIDC identity provider — Keycloak,
Okta, Entra ID, Auth0, Google Workspace — and turn the groups that provider
reports into rolter roles.

Single sign-on is **optional and additive**. A deployment that never registers a
provider behaves exactly as it did before: local accounts, passwords, and
operator-granted roles. Nothing about SSO is reachable, and the login screen
never mentions it.

## The three supported deployments

| Deployment | Configuration | Login screen |
| --- | --- | --- |
| Local accounts only | no `sso_providers` rows | email + password |
| SSO only | a provider, and `allow_password_login = false` on the org | one "Continue with …" button |
| Both | a provider, and password login left enabled | password form *and* the button |

The dashboard asks `GET /api/v1/auth/methods` — the one unauthenticated
endpoint in this area — and renders whichever of the three it is told. That
endpoint returns provider names, slugs and start URLs only; all of which are
already visible in the login URL, and none of which are secret.

## The flow

Authorization code with PKCE, no implicit grant, no client-side tokens:

1. `GET /auth/sso/{slug}/start` mints a `state`, a `nonce` and a PKCE verifier,
   stores them in `sso_login_states`, and redirects to the provider's
   `authorization_endpoint`.
2. The provider redirects back to `GET /auth/sso/{slug}/callback`.
3. The callback **consumes** the state row (`DELETE … RETURNING`), so a replayed
   `code` + `state` pair finds nothing and is refused. States older than ten
   minutes are treated as absent and swept.
4. The code is exchanged at the `token_endpoint` with the PKCE verifier and the
   sealed client secret.
5. The id token is verified against the provider's JWKS: signature by `kid`,
   issuer, audience (`client_id`), expiry, and the `nonce` from step 1.

Rules that hold on every path:

- The **redirect URI comes from configuration** (`ROLTER_PUBLIC_URL`), never
  from the request, so an attacker cannot point the callback elsewhere.
- Only asymmetric algorithms are accepted (RS256/384/512, ES256/384, PS256).
  `HS256` and `none` are rejected — a symmetric id token signed with a value the
  attacker may know is not evidence of anything.
- The discovery document's `issuer` must equal the configured issuer.
- A failed token exchange reports the HTTP status only. The provider's error
  body can echo the client secret back, and that must not reach a log.

## Groups become memberships

An operator maps an IdP group to a role at an org, team or project scope:

```http
POST /api/v1/sso-providers/{id}/group-mappings
{"group_name": "platform", "role": "admin", "team_id": "…"}
```

The `groups` claim is read in every shape providers actually send it — a JSON
array, a single string, or a space-separated list — and Keycloak's leading `/`
is stripped, so operators map the group name they see in the IdP's own UI. The
claim name is configurable per provider (`group_claim`, default `groups`).

If no mapping matches, the provider's `default_role` applies. If there is no
`default_role` either, **the login is refused**: SSO authenticates, it does not
implicitly authorize.

### Manual and SSO grants co-exist

Every membership records where it came from:

- `source = 'manual'` — an invitation, the admin API, the seed command.
- `source = 'sso'` — an IdP group mapping.

Each login reconciles **only the `sso` rows inside that provider's org**. So:

- A role an operator granted by hand survives every SSO login, forever.
- Dropping a user from a mapped group revokes the role that group granted, on
  their next login — including the login that is then refused for having no
  grants left. That is how deprovisioning through the IdP works without SCIM.
- Another org's SSO grants are untouched; they belong to that org's provider.

An account is adopted **by verified email**: someone invited last month who now
arrives through the IdP keeps the same user row, the same virtual keys and the
same manual roles. SSO-created accounts get no password — an SSO identity must
not silently gain a second, weaker credential.

## Org login policy

`PUT /api/v1/orgs/{org_id}/auth-policy` (org admin) sets two flags:

- `allow_password_login` — when false, members of this org cannot use the
  password form.
- `allow_sso` — when false, callbacks for this org's providers are refused
  without deleting the provider rows, so an IdP can be cut off in one request.

Two guard rails, both returning `409`:

- Both flags off is not a policy, it is an outage.
- Password login cannot be disabled before an enabled provider exists.

And one exemption: **a superadmin can always log in with a password**, whatever
the policy says. A mistyped issuer or an IdP outage would otherwise lock the
deployment out with no way back in. That exemption is the reason the flag is
safe to turn on at all; keep the superadmin's password strong and stored
somewhere the IdP does not gate.

## Configuration

| Setting | Where | Notes |
| --- | --- | --- |
| `ROLTER_PUBLIC_URL` | env | the control plane's externally reachable base URL; the redirect URI is derived from it. Defaults to `http://localhost:4001`, and is read once at startup so a change needs a restart |
| `ROLTER_KEK` | env | required to store or read a client secret; the secret is sealed with AES-256-GCM exactly like provider credentials |
| `ROLTER_SESSION_PEPPER` | env | session tokens are stored as peppered digests, same as local logins |

Register the redirect URI `"$ROLTER_PUBLIC_URL/auth/sso/{slug}/callback"` with
the identity provider.

## Testing

Unit and Postgres-gated integration tests drive a stub IdP in-process, which
covers rolter's own logic. Interoperability is a separate question, so the
[e2e harness](../../integration/e2e/README.md) runs the same flows against a
real Keycloak — genuine discovery document, real JWKS, real login form, real
`/`-prefixed realm groups:

```bash
cd integration/e2e && uv run pytest tests/test_sso.py --idp
```

That suite is nightly and on-demand (`.github/workflows/sso-e2e.yml`), not part
of the per-PR gate.

## Related

- [RBAC & auth](rbac-and-auth.md) — roles, scopes and how they are enforced
- [SCIM provisioning](scim-provisioning.md) — IdP-driven account lifecycle,
  which pairs with SSO but is independent of it
- [Security](security.md) — secret handling and the threat model
