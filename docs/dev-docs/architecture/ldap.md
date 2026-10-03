# LDAP authentication

> **Status: not shipped.** LDAP sign-in does not work in any build. The provider
> in `crates/rolter-control/src/ldap.rs` is implemented and unit-tested, but
> nothing constructs it: there is no configuration key, environment variable,
> route or dashboard screen for a directory, and `POST /api/v1/auth/login`
> resolves only local accounts. #2731 decided to keep the module, marked as
> unwired, rather than delete it, because wiring it (#1826) reuses the policy
> described below. The configuration on this page is the provider's struct
> shape, not a key rolter reads. Do not document LDAP as available to users
> until #1826 lands.

What wiring needs, as #1826 scopes it: somewhere to keep the directory settings
with the service-account password sealed under the KEK (so a table, a
`SEALED_COLUMNS` entry and a Governance screen next to Single Sign-On),
dispatch from the password login to the directory, group mapping through the
same mapping model SSO and SCIM use rather than the provider's own
`group_role_map`, `GET /api/v1/auth/methods` reporting it, and a missing
`display_name` defaulted from the directory the way an OIDC first sign-in
defaults it.

The intended third identity provider behind `rolter_auth::IdentityProvider`,
after local password login and OIDC SSO (#241). It is compiled in behind the
`ldap` cargo feature.

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

`group_role_map` maps a directory group onto a rolter role. It is explicit and
additive:

- a user in several mapped groups gets the **union** of their roles;
- a user in **no** mapped group is **rejected**, even with a correct password.

There is no implicit default role and no wildcard. Authenticating proves who
someone is, not what they may do — the same rule SSO already follows.

Group names are matched case-insensitively and with surrounding whitespace
trimmed, because directories are inconsistent about DN case and an operator who
copies a group from their directory's UI should not silently get no access.

## What a client is told

Every authentication failure returns the same generic rejection:

| Situation                             | Result                              |
| ------------------------------------- | ----------------------------------- |
| No such user                          | `NotVerified`                       |
| Wrong password                        | `NotVerified`                       |
| User outside `base_dn`                | `NotVerified`                       |
| Authenticated, but in no mapped group | `NotVerified`                       |
| Directory unreachable or misbehaving  | `Provider("directory unavailable")` |

The first four are deliberately indistinguishable. A login form that
distinguishes "no such user" from "wrong password" is a user-enumeration oracle
against the corporate directory, which is a worse leak than the login itself.

A directory being _down_ is distinguishable, because that is an operational
fact the operator needs and reveals nothing about any account. Distinguished
names, filters, bind credentials and raw LDAP result codes are logged for the
operator and never returned.

## Configuration

```toml
url = "ldaps://dir.example.com"          # ldaps:// strongly preferred
bind_dn = "cn=svc-rolter,dc=example,dc=com"
bind_password = "…"                       # read-only service account
base_dn = "ou=people,dc=example,dc=com"
user_filter = "(uid={login})"             # must contain {login}
email_attr = "mail"
display_name_attr = "cn"
group_attr = "memberOf"
require_tls = true

[group_role_map]
"cn=rolter-admins,ou=groups,dc=example,dc=com" = "admin"
"cn=rolter-users,ou=groups,dc=example,dc=com" = "member"
```

Configuration is validated before it is used to authenticate anyone. These are
rejected at startup rather than at first login:

- a URL that is neither `ldap://` nor `ldaps://`;
- `require_tls` with a plain `ldap://` URL — the service-account bind password
  would cross the network in cleartext;
- a `user_filter` without `{login}`, which would authenticate whoever the filter
  happens to match, for any login submitted;
- an empty `base_dn`;
- an empty `group_role_map`, under which no directory user could ever be granted
  a role.

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
