# LDAP authentication

LDAP sign-in lets a directory user sign in to the dashboard and the API with
the directory password through the ordinary login form (#241, wired by #1826).
It is the third `rolter_auth::IdentityProvider`, after local password login and
OIDC SSO, and it is compiled in behind the `ldap` cargo feature of
`rolter-control` (`cargo build --features postgres,ldap`; a build without it
refuses to store a directory rather than accept settings nothing could use).

The directory half — the two-phase bind, filter escaping, error normalization —
is `crates/rolter-control/src/ldap.rs`. Everything around it is
`crates/rolter-control/src/ldap_signin.rs`: the stored settings, their admin API,
and the step `POST /api/v1/auth/login` takes through a directory.

## Where it sits in sign-in

The login form is unchanged. The local account is tried first; when its password
does not verify (and only then — an account an org forces through SSO keeps its
own answer) and at least one directory is enabled, each enabled directory is
asked in turn and the first that authenticates the login wins. The identity it
produces is the local account's, so everything after it — the second-factor
policy, the throttle, the session, the audit entry — is the code a local
password sign-in runs. A second factor required by the org applies to a
directory user exactly as to a local one.

`GET /api/v1/auth/methods` reports `ldap: true` while a directory is enabled and
the build can reach one. `password` is then true as well, since a directory user
types into the password form.

A directory that is down is logged for the operator and answered like a wrong
password. Local sign-in is unaffected, and a local account's own failed login
does not turn into a 500 because an unrelated directory is unreachable.

## Storage

Migration `0104_ldap_directories.sql` adds two tables, neither read by the data
plane (so, like the OIDC tables, neither carries a `bump_config_version()`
trigger and neither may grow one):

- `ldap_directories` — at most one per org (`ldap_directories_org_unique`),
  because a sign-in has no org in hand and one directory per org keeps the grants
  it produces attributable to one reconciliation. The service-account password is
  sealed with the deployment KEK into `bind_secret_ciphertext` + `bind_secret_nonce`
  (both null or both set) and is listed in `SEALED_COLUMNS`; the API serializes it
  as `has_bind_password` and never returns it. A null `default_role` refuses a
  user in no mapped group.
- `ldap_group_mappings` — a directory group granting `admin`, `member` or
  `viewer` at the org, a team or a project, with the same "most specific non-null
  id" convention as `sso_group_mappings` and `scim_group_mappings`.

It also widens `memberships_source_check` with `ldap`. The memberships a sign-in
produces carry that source and are reconciled on every sign-in through
`sso::reconcile_grants`, the function OIDC sign-in uses with `sso`: grants of
another source (`manual`, `sso`, `scim`) are never touched, and an org's last
admin grant is kept and audited rather than failing the sign-in (#2558).

## Admin API

| Method and path                                     | Capability                  |
| --------------------------------------------------- | --------------------------- |
| `POST /api/v1/orgs/{org_id}/ldap-directories`       | `ldap_directory` create     |
| `GET /api/v1/orgs/{org_id}/ldap-directories`        | `ldap_directory` read       |
| `PUT /api/v1/ldap-directories/{id}`                 | `ldap_directory` update     |
| `DELETE /api/v1/ldap-directories/{id}`              | `ldap_directory` delete     |
| `POST /api/v1/ldap-directories/{id}/group-mappings` | `ldap_group_mapping` create |
| `GET /api/v1/ldap-directories/{id}/group-mappings`  | `ldap_group_mapping` read   |
| `DELETE /api/v1/ldap-group-mappings/{id}`           | `ldap_group_mapping` delete |

All of them need the org `admin` role. The bind password is write-only: absent on
an update keeps it, an empty string clears it (an anonymous search), a value
replaces it. A write is refused at the point of the mistake — an unknown scheme,
plain `ldap://` while `require_tls` is on, a filter without `{login}`, an empty
base, a URL the egress policy denies, an unknown role, or a build without the
`ldap` feature. A second directory for an org is a `409`, and a mapping may only
grant inside the directory's own org. The audit trail records `ldap_directory.*`
and `ldap_group_mapping.*` events (a password rotation is recorded as
"rotated", never its value) and each directory sign-in records
`auth.ldap_sign_in` with the roles in force.

## How a login is verified

A user's distinguished name is rarely their login name, so verification is a
two-phase bind:

1. **Find.** Bind as a read-only service account and search `base_dn` for the
   entry matching `user_filter`, with `{login}` substituted.
2. **Verify.** Bind a second time _as the entry that was found_, using the
   password the user submitted. That bind is the authentication.

rolter never reads, compares or stores a password hash from the directory. It
also never caches the password.

A filter matching more than one entry is a hard failure rather than a login:
binding as an arbitrary one of several matches would authenticate the wrong
person.

## Group mapping is least-privilege

Group mapping goes through the same scoped-grant model SSO and SCIM use rather
than a model of its own. It is explicit and additive:

- a user in several mapped groups gets the **union** of their grants;
- a user in **no** mapped group gets the directory's `default_role` at the org,
  and is **rejected** when there is none, even with a correct password;
- dropping a user from a mapped group revokes what it granted the next time they
  sign in (the same moment an OIDC sign-in reconciles), while a role an
  administrator granted by hand is never touched.

There is no wildcard mapping. Authenticating proves who someone is, not what they
may do — the same rule SSO already follows.

Group names are matched case-insensitively and with surrounding whitespace
trimmed, because directories are inconsistent about DN case and an operator who
copies a group from their directory's UI should not silently get no access.

## What an LDAP sign-in refuses

- **A superadmin is never adopted.** An entry whose mail matches a superadmin
  account does not sign in as it: the break-glass account has to stay reachable
  when the directory is down, and must not be reachable by whoever can edit a
  directory attribute. Superadmins sign in locally.
- **An account holding a deployment role is never adopted.** An entry whose
  mail matches an account with `users.deployment_role` set (operator, security
  auditor) does not sign in as it, even when the account has no memberships:
  that authority is deployment-wide and lives on the account, so whoever can
  edit a directory attribute must not reach it. These accounts sign in locally.
- **An account with a role in another org is never adopted.** The directory is
  run by an org's admins, who choose every entry's mail and password, so it may
  sign in as an existing account only when every role that account holds is
  inside the directory's own org.
- **Not while the org has password login off.** A directory sign-in is a
  password typed into rolter's form, so a directory whose org sets
  `allow_password_login = false` is skipped.
- **A deactivated account stays out**, whatever the directory says.
- **No mail, no account.** Accounts are keyed on email across providers, so an
  entry that yields no usable address (and a login that is not one) is refused
  rather than given a made-up one.

An account that signs in for the first time is created without a local password
and, like an OIDC first sign-in, gets its display name from the directory's name
attribute when it has none (`UserRepo::default_display_name`, which never
replaces an existing name).

## What a client is told

Every authentication failure returns the same generic rejection:

| Situation                             | Result                                       |
| ------------------------------------- | -------------------------------------------- |
| No such user                          | `NotVerified`                                |
| Wrong password                        | `NotVerified`                                |
| User outside `base_dn`                | `NotVerified`                                |
| Authenticated, but in no mapped group | `NotVerified` (after the caller maps groups) |
| Directory unreachable or misbehaving  | `Provider("directory unavailable")`          |

The first four are deliberately indistinguishable. A login form that
distinguishes "no such user" from "wrong password" is a user-enumeration oracle
against the corporate directory, which is a worse leak than the login itself.

A directory being _down_ is distinguishable, because that is an operational
fact the operator needs and reveals nothing about any account. Distinguished
names, filters, bind credentials and raw LDAP result codes are logged for the
operator and never returned.

## Configuration

A directory is configured through the API above, not a config file. The fields:

```json
{
  "name": "Corporate directory",
  "url": "ldaps://dir.example.com",
  "bind_dn": "cn=svc-rolter,dc=example,dc=com",
  "bind_password": "…",
  "base_dn": "ou=people,dc=example,dc=com",
  "user_filter": "(|(uid={login})(mail={login}))",
  "email_attr": "mail",
  "display_name_attr": "cn",
  "group_attr": "memberOf",
  "require_tls": true,
  "default_role": null,
  "enabled": true
}
```

Sealing the bind password needs `ROLTER_KEK`; without it a request carrying one
is refused with `503`, and a stored directory whose password cannot be opened is
skipped at sign-in with a warning.

Settings are validated before they are stored and again before they are used to
authenticate anyone:

- a URL that is neither `ldap://` nor `ldaps://`;
- `require_tls` with a plain `ldap://` URL — the service-account bind password
  would cross the network in cleartext;
- a `user_filter` without `{login}`, which would authenticate whoever the filter
  happens to match, for any login submitted;
- an empty `base_dn`;
- a URL whose host literal the deployment's egress policy denies.

Directory operations time out after ten seconds, so a directory that accepts a
connection and goes quiet cannot hold a sign-in open.

## Filter injection

The login is escaped per RFC 4515 before substitution. Without it a login of `*`
matches every user in the subtree, and `)(uid=admin` rewrites the filter — LDAP
injection, the directory analogue of SQL injection. `\`, `*`, `(`, `)`, `/` and
NUL are escaped.

## Operational limitations

- **Group membership is read from the user entry** via `group_attr` (typically
  `memberOf`). Directories that do not populate a reverse-membership attribute
  need a nested group search, which is not implemented.
- **Nested groups are not expanded.** A user in a group that is itself a member
  of a mapped group is not granted that role. Map the groups users are directly
  in.
- **No connection pooling.** Each login opens and closes its connections. This
  is correct but not fast; an interactive login is not a hot path, and pooling
  bound connections has its own correctness hazards.
- **No referral chasing.** A referral from the directory is not followed.
- **`ldaps://` only for transport security.** StartTLS on a plain `ldap://`
  connection is not implemented.

## Dashboard

Governance › LDAP is `ui/src/pages/Ldap.tsx`, over `ui/src/lib/api/ldap.ts`. It is gated on `ldap_directory` (the screen, edit, enable, delete) and `ldap_group_mapping` (the mapping form, through the shared `GroupMappings` with `kind="ldap"`). An org has one directory, so the add button is disabled once one exists; a `409` from a racing second registration is shown in the sheet. The bind password follows the SSO client-secret rule: the edit sheet omits `bind_password` when the field is empty (keep) and sends the typed value (rotate); clearing is a separate confirmed control on the card, the only place that sends `""`.
