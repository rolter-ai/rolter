# Single sign-on (OIDC)

rolter can authenticate operators against an OIDC identity provider — Keycloak,
Okta, Entra ID, Auth0, Google Workspace — and turn the groups that provider
reports into rolter roles.

Single sign-on is **optional and additive**. A deployment that never registers a
provider behaves exactly as it did before: local accounts, passwords, and
operator-granted roles. Nothing about SSO is reachable, and the login screen
never mentions it.

## The three supported deployments

| Deployment          | Configuration                                             | Login screen                   |
| ------------------- | --------------------------------------------------------- | ------------------------------ |
| Local accounts only | no `sso_providers` rows                                   | email + password               |
| SSO only            | a provider, and `allow_password_login = false` on the org | one "Continue with …" button   |
| Both                | a provider, and password login left enabled               | password form _and_ the button |

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
  An account a provider created has no password, so while this is off those
  members cannot sign in at all, whatever `allow_password_login` says. The
  dashboard confirms the change when the org has an enabled provider (#2326).

Three guard rails, all returning `409`:

- Both flags off is not a policy, it is an outage.
- Password login cannot be disabled before an enabled provider exists.
- The inverse: while password login is off, the org's last enabled provider can
  be neither disabled (`PUT /sso-providers/{id}` with `enabled: false`) nor
  deleted (#2233). The superadmin exemption below would still let someone in,
  but the guard is about every other member.

Both directions are checked per org, inside the write's transaction, under one
`pg_advisory_xact_lock` keyed on the org (`lock_org_sign_in` in the store). A
row lock on either table could not order a provider write against a policy
write, and without the lock two concurrent disables of different providers
would each see the other still enabled.

And one exemption: **a superadmin can always log in with a password**, whatever
the policy says. A mistyped issuer or an IdP outage would otherwise lock the
deployment out with no way back in. That exemption is the reason the flag is
safe to turn on at all; keep the superadmin's password strong and stored
somewhere the IdP does not gate.

## Configuration

| Setting                 | Where | Notes                                                                                                                                                                                |
| ----------------------- | ----- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `ROLTER_PUBLIC_URL`     | env   | the control plane's externally reachable base URL; the redirect URI is derived from it. Defaults to `http://localhost:4001`, and is read once at startup so a change needs a restart |
| `ROLTER_KEK`            | env   | required to store or read a client secret; the secret is sealed with AES-256-GCM exactly like provider credentials                                                                   |
| `ROLTER_SESSION_PEPPER` | env   | session tokens are stored as peppered digests, same as local logins                                                                                                                  |

Register the redirect URI `"$ROLTER_PUBLIC_URL/auth/sso/{slug}/callback"` with
the identity provider.

A provider's slug is in that URI, so it is fixed at creation (`PUT` does not
accept one) and has one rule, `^[a-z0-9][a-z0-9-]{0,62}$`: lowercase letters,
digits and hyphens, starting with a letter or digit, at most 63 characters. It
is enforced in three places that have to agree (#2304):

- the `sso_providers_slug_charset` check constraint in
  `0047_sso_providers.sql`, which is the rule itself;
- `validate_slug()` in `sso.rs`, which `create_provider` runs before the insert
  so a bad slug is a `400` that states the rule and not a store error carrying
  the constraint name. The slug is checked as sent, never trimmed or lowercased;
- `ui/src/lib/sso-slug.ts`, which the add sheet uses to mark the field, hide the
  redirect URI preview and block Save. It never rewrites the input, because the
  slug is registered at the identity provider and the admin has to see exactly
  what will be saved; it only suggests a corrected value.

`sso_slug_outside_the_charset_is_a_400_that_states_the_rule` in
`crates/rolter-control/tests/control_integration.rs` asks the store and the
endpoint about the same table of slugs and requires the same answer, and
`sso-slug.test.ts` reads the migration and compares the pattern, so a change to
one is caught by the others. Widening the rule means a new migration (the
applied one is never edited) plus both mirrors.

The dashboard never assembles that URI, or the login URL, from the browser's
origin (#2083). Every provider row the admin API returns carries both, built by
`redirect_uri()` and `login_url()` in `sso.rs` (the functions `start_login`
itself uses), so the value on the card is byte for byte the one the flow sends:

```json
{
  "slug": "okta",
  "redirect_uri": "https://rolter.example.com/auth/sso/okta/callback",
  "login_url": "https://rolter.example.com/auth/sso/okta/start"
}
```

The identity provider wants the redirect URI before the provider exists in
rolter, since it issues the client id and secret the add form asks for. So the
add sheet previews it from the slug as it is typed, on the base
`GET /api/v1/public-url` returns (`public_url.rs`, capability `public_url`,
readable by any authenticated caller), with `ssoRedirectUri()` in
`ui/src/lib/api.ts` mirroring the Rust path. One read serves every keystroke.
The endpoint also reports `configured: false` when `ROLTER_PUBLIC_URL` is unset,
and the screen warns that the default only reaches rolter from a browser on the
control plane's own host. It is deployment-wide rather than SSO-specific so the
User Provisioning screen can build its SCIM base URL from the same value (#2079).
Both screens read it through `usePublicUrl()` (`ui/src/lib/use-public-url.ts`),
the one place its query key and options are written, so they share one request.

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
