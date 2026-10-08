# SCIM 2.0 provisioning

An identity provider can create, update, deactivate and reconcile rolter accounts _and groups_ over SCIM 2.0 instead of an operator doing it by hand. Users answer _who exists_; groups, through an operator-written mapping, answer _what they may do_.

## Tokens carry the tenant

There is no org id anywhere in the SCIM path. An operator mints an org-scoped provisioning token, and that token is what resolves the tenant:

```
POST   /api/v1/orgs/{org_id}/scim-tokens   { "name": "okta" }   # org admin
GET    /api/v1/orgs/{org_id}/scim-tokens
DELETE /api/v1/scim-tokens/{id}
```

The plaintext token (`rolter_scim_…`) is returned **once**, at creation. Only its peppered SHA-256 digest is stored — the same treatment as session tokens — so a database dump alone cannot be replayed, and a lost token is rotated rather than looked up. Revoking takes effect on the very next request; there is no cache to invalidate.

Because the token resolves to exactly one org, an IdP cannot address another tenant's users: a foreign resource id is a `404`, not a redacted row.

## The base URL an IdP is pointed at

The resource endpoints below are mounted at the root of the control plane (`scim.rs`, `scim_groups.rs`), so the base URL a connector needs is the control plane's public URL plus `/scim/v2`. The control plane builds every address it gives an outside caller from `ROLTER_PUBLIC_URL`, never from the request, and the dashboard can be open under another name than the one the IdP must call, so the User Provisioning screen does not derive the URL from `window.location` (#2079).

It reads `GET /api/v1/public-url` (`public_url.rs`, capability `public_url`, readable by any signed-in caller) and appends the path with `scimBaseUrl()` in `ui/src/lib/api.ts`, which mirrors the mount; `ui/src/lib/scim-urls.test.ts` pins the pair. The read goes through `usePublicUrl()`, which the [Single Sign-On](sso.md) screen calls too, so both screens share one request and agree on whether the variable is set.

The address is shown twice with a copy button each: above the token list, and in the reveal step beside the token, where an operator holds a secret that is never shown again. The reveal is a sheet rather than a dialog for that reason, and it takes the shared `SecretValue` and close guard: a failed copy leaves a message under the token, and closing the sheet before the token was copied asks first ([Dashboard one-time secrets](../development/secret-reveal.md)). With `configured: false` the control plane's default `http://localhost:4001` stands in for the variable. The screen still shows it and says, under it, that an IdP can reach it only from the control plane's own host. The address appears only beside a token list that was read, since a caller refused the list or a control plane with no store has no connector to point anywhere. A failed read of the public URL shows a `LoadError` with a retry instead of a guessed host.

## Users

```
GET    /scim/v2/Users?filter=userName eq "ada@example.com"
POST   /scim/v2/Users
GET    /scim/v2/Users/{id}
PUT    /scim/v2/Users/{id}
PATCH  /scim/v2/Users/{id}
DELETE /scim/v2/Users/{id}
```

- **Filters.** Only `userName eq "value"` is supported — the one shape reconciliation needs. Any other filter is a `400` with `scimType: invalidFilter` rather than being ignored: silently returning the whole directory reads to an IdP as "no such user", and it then re-creates the account.
- **Idempotence.** IdPs retry. Creating an existing `userName` returns `409` with `scimType: uniqueness` instead of a second account. If the email already belongs to a local account (invited by an admin, or provisioned elsewhere), that account is adopted rather than failing on the unique email, so provisioning converges.
- **PATCH.** `active` and `displayName` are implemented, in both the `path: "active"` / `path: "displayName"` and bare `{"active": false, "displayName": "Ada"}` forms, and with the string `"true"`/`"false"` some IdPs send for `active`. Any other attribute or operation is a `400` — an IdP that gets a success for an operation nothing applied would believe the change landed. Every operation is read before anything is written, and `active` is applied before the name, so a deactivation that is refused (the last superadmin) renames nothing. `displayName` goes through the same `sync_display_name` helper as `POST` and `PUT`, so it is sanitised the same way and a name that sanitises to nothing leaves the stored one alone.
- **Two orgs, one name.** `users.display_name` is one column per account, while a SCIM identity is per org, so an account provisioned by two orgs has two `displayName` values and one name. The rule is last sync wins: whichever org's `POST`, `PUT` or `PATCH` ran most recently decides what the dashboard shows. There is no per-org precedence and no merge, because an account belongs to a person rather than an org and a name has nowhere else to live. Each org's `GET /scim/v2/Users/{id}` still echoes the `displayName` its own IdP last sent, taken from `scim_identities`. The account is `display_name_managed` while any org holds an identity for it.
- **Deactivation logs the user out.** Setting `active: false` stamps `deactivated_at` _and_ deletes the account's live sessions, because an IdP disabling a leaver expects them out now, not merely unable to log in again.
- **`displayName` is echoed raw.** The Users resource returns `displayName` from `scim_identities`, exactly as the IdP last sent it, while `users.display_name` holds the sanitised copy (`me::sanitise_directory_name`: control characters stripped, trimmed, capped). This is deliberate (#2731): an IdP reconciles by comparing what it pushed with what comes back, so a trimmed echo reads as drift and is re-pushed on every sync; the account column is shared by every org that provisions the person, so it is not this org's value; and a name that sanitises to nothing leaves the account column unchanged, which would echo a name the IdP never sent. The raw value only leaves as an escaped JSON string, to the IdP that wrote it.
- **DELETE deprovisions.** The account is deactivated, its sessions dropped, and the org's SCIM identity mapping removed. The `users` row itself stays: its memberships and audit trail must outlive any one IdP, and a later `POST` adopts it again.

## No password path

A `password` attribute in a SCIM body is ignored, not honoured. Provisioned accounts are SSO-shaped: `password_hash` stays null, and there is no code path through which SCIM can set or read a local credential.

## Groups

```
GET    /scim/v2/Groups?filter=displayName eq "platform"
POST   /scim/v2/Groups
GET    /scim/v2/Groups/{id}
PUT    /scim/v2/Groups/{id}
PATCH  /scim/v2/Groups/{id}
DELETE /scim/v2/Groups/{id}
```

Same token, same tenancy rule, same `ScimError` envelope as Users.

- **Filters.** Only `displayName eq "value"` is supported, for the same reason `userName eq` is the only Users filter: answering an unsupported filter with the whole directory reads to an IdP as "no such group", and it then creates a duplicate.
- **Members must already be provisioned here.** A `members` entry has to be a user with a SCIM identity in the token's org. Anything else — an unparsable id, a local account the IdP never created, another tenant's user — is a `400` with `scimType: invalidValue`. A group can therefore never be a side door into an account the Users surface did not make.
- **PATCH.** The operations IdPs actually send are implemented: `add`/`replace` on `members` (array of `{"value": id}`, array of bare ids, or a single entry), `remove` on `members` both with a value list and in the filtered `members[value eq "…"]` path form Okta uses, `remove` on the whole attribute to empty the group, `replace` on `displayName`/`externalId`, and the pathless `{"op": "replace", "value": {…}}` shape. Anything else is a `400`.
- **Idempotence.** A replayed create is a `409` with `scimType: uniqueness`. A replayed `add` of an existing member, or a `PUT` re-sending the same member list, changes nothing.
- **DELETE.** The group row goes and its members are reconciled in the same request, so the roles it granted disappear immediately rather than at the next sync. A second `DELETE` is a `404`.

## Group→team mapping

A mapping is written by an operator, not by the IdP — the IdP may not decide what its groups are worth:

```
POST   /api/v1/orgs/{org_id}/scim-group-mappings   # org admin
       { "group_name": "platform", "role": "member", "team_id": "…" }
GET    /api/v1/orgs/{org_id}/scim-group-mappings
DELETE /api/v1/scim-group-mappings/{id}
```

This is deliberately the [SSO group-mapping model](sso.md), down to the vocabulary: a `group_name` grants one of `admin`/`member`/`viewer` at the most specific non-null scope id (`team_id`, `project_id`, or the org itself when neither is named), and a mapping may only grant inside its own org — a team or project belonging to another tenant is a `400`. Two divergent group-mapping models in one control plane would be two things for an operator to learn and two places for a privilege bug to hide.

Mappings key on the group's `displayName`, which is what an operator sees in the IdP UI. Renaming a group in the IdP therefore detaches it from a mapping written against the old name; rewrite the mapping, or keep the SCIM display name stable.

A mapping may be written before the IdP has ever mentioned the group. Creating or deleting one reconciles that group's current members straight away, so a mapping change does not wait for the next sync.

## Reconciliation converges

Every group write recomputes the affected users' memberships from the database, never from the request body. The wanted set is "every mapping of every group this user is currently in"; `source = 'scim'` rows that are not in it are deleted, missing ones are created. Three consequences:

- A scheduled sync re-sending the same group state is a no-op.
- Dropping a user from a group revokes exactly what that group granted, in the same request.
- A grant an operator made by hand carries `source = 'manual'` and is never touched — the same rule SSO logins follow, so the enrolment paths can be used side by side. An equivalent manual grant also suppresses creating a duplicate `scim` row.

`DELETE /scim/v2/Users/{id}` drops the account from every group in the org before removing the identity, so a deprovisioned account cannot keep a group-granted role.

## Roles

A provisioned account is granted a **viewer** membership at the org it was provisioned into, merely for existing — least privilege on purpose. Anything beyond a read comes from a group mapping an operator wrote.

## Audit

`scim.user.create`, `scim.user.update`, `scim.user.deprovision`, `scim.group.create`, `scim.group.update` and `scim.group.delete` are written to `audit_log`, scoped to the org, with the provisioning token's id in the detail — the actor is a token, not a human. Operator actions carry the acting operator instead: `scim_token.create` / `scim_token.revoke`, and `scim_group_mapping.create` / `scim_group_mapping.delete`. `scim_tokens.created_by` is filled the same way, from `Principal::account_id()` (#2844), so a superadmin session with no membership in the org is recorded and the admin token or open mode leaves it null; see [who an audit row names as the actor](rbac-and-auth.md#who-an-audit-row-names-as-the-actor-2844).
