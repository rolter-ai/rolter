# Two-factor authentication

TOTP (RFC 6238) as a second factor on local-account login (#1078). The
operator-facing guide is
[`user-docs/security/two-factor-authentication.mdx`](../../user-docs/security/two-factor-authentication.mdx);
this page is the design and the reasons behind it.

## Where the factor is checked

Once, on the login exchange, and nowhere else.

A session is the unit of trust in this control plane (see
[RBAC & auth](rbac-and-auth.md)): an opaque bearer token backed by a row,
revocable immediately, presented on every request. Re-checking a factor per
request would mean either keeping the secret somewhere a request path can
reach it, or asking the user for six digits every few seconds — and it would
buy nothing that revoking the session does not already give.

So `POST /api/v1/auth/login` does not issue a session when the account has an
armed factor. It issues a **challenge**: a separate short-lived token that
authenticates nothing and names only which login is in flight.
`POST /api/v1/auth/mfa/verify` redeems it for a real session.

```
password ok ──► armed factor?
                 │
                 ├── no, and no policy requires one ──► session
                 ├── no, but policy requires one ─────► 403 mfa_enrolment_required
                 └── yes ──► challenge ──► /auth/mfa/verify ──► session
```

The challenge lives in its own table rather than as a flagged `sessions` row.
A challenge grants nothing, and putting it in `sessions` would mean every
future reader of that table had to remember to exclude it — a rule that holds
only until someone forgets it.

## Why SHA-1

RFC 6238 permits SHA-1, SHA-256 and SHA-512, and rolter uses SHA-1. Not
because it is the safe default in general, but because it is the only
interoperable one here: Google Authenticator ignores the `algorithm` parameter
in the `otpauth://` URI and computes SHA-1 regardless, so arming SHA-256 would
hand a large share of operators a factor that silently never verifies.

The construction is HMAC rather than a bare digest, so SHA-1's collision
weaknesses do not apply, and the secret is 160 bits of CSPRNG output. The
practical attack on a TOTP factor is guessing a six-digit code, which is what
the replay and attempt rules below defend.

## Replay

A TOTP code is valid for its whole 30-second step, and rolter accepts one step
either side to absorb clock skew — so without further defence, one
shoulder-surfed code stays usable for up to 90 seconds.

`user_totp_factors.last_used_step` records the highest step the factor has
accepted, and a verification is accepted only for a step **strictly greater**
than it. The comparison is a `where` clause on the update, not a read followed
by a decision in Rust: two logins racing with the same stolen code would both
read the same value and both conclude it was fresh, whereas as a conditional
update exactly one of them affects a row.

The visible consequence is that the code used to *confirm* an enrolment is
spent, so the first sign-in afterwards needs the next code. That is the rule
working. It is called out in the operator docs so it does not read as a fault.

## Guessing

Two independent budgets, because they defend different things.

- **The password** is guarded by [`login_throttle`](../../crates/rolter-control/src/login_throttle.rs)
  (#1079), unchanged: a wrong password still costs the attacker its escalating
  delay and lockout.
- **The challenge** carries its own `attempts` counter, capped at three, and
  the attempt is charged in the same statement that reads the challenge — so a
  caller cannot buy extra guesses by issuing requests in parallel. Once the
  budget is spent the challenge is dead to the correct code too, which is what
  stops "hold one challenge open and grind" from working.

The throttle counter is cleared when the password succeeds, before the
step-up. Holding the failed-password lock open across the challenge would make
a wrong code look like a wrong password and lock the account on the strength
of a mistyped digit.

## Storage

| Table | Holds |
|---|---|
| `user_totp_factors` | one row per user: the sealed secret, `confirmed_at`, `last_used_step` |
| `user_recovery_codes` | hashed single-use codes; `used_at` marks a spent one |
| `mfa_challenges` | logins in flight; hashed token, attempt count, expiry |
| `org_auth_policies.mfa_policy` | enforcement, alongside the password/SSO switches |

The secret is sealed with the deployment KEK — it is a bearer credential, so a
database dump alone must not yield one — and is registered in `SEALED_COLUMNS`
so `rolter kek verify` reports a restore that cannot open it. Recovery codes
and challenge tokens are peppered digests, the same construction virtual keys
and session tokens use.

There is no model type that serialises the secret. The only path that unseals
it is `MfaRepo::open_secret`, which hands back a non-`Serialize` struct;
`MfaRepo::status` describes the factor without it, and that is what the API
returns.

None of these tables carries a `bump_config_version()` trigger, and none may
grow one: the data plane never reads them, so there is nothing for
`/internal/snapshot` to propagate, and bumping the version on every enrolment
would wake the whole gateway fleet for a change it cannot observe. Same
reasoning as `custom_roles` in `0058`.

## Enforcement

`mfa_policy` is `off` / `optional` / `required_superadmin` / `required_all`,
and a user's effective policy is the **strictest** across their orgs. Taking
the first match instead would let a relaxed membership soften a hardened one.

Under a `required_*` policy an unenrolled account is refused a session rather
than admitted unprotected, with a distinct `mfa_enrolment_required` code — the
remedy is an administrator's, and telling the user to retype their password
would be a lie.

Unlike `allow_password_login`, `required_all` does not exempt superadmins. The
exemption there exists because a broken IdP has no other way back in; here the
way back in is `rolter mfa reset`, which needs no exemption to work.

## Break-glass

`rolter mfa reset --email ... --reason ...` clears the factor and its recovery
codes, revokes every live session for the account, and writes an audit entry
carrying the reason. It runs on the host against the database, which is the
same privilege level as reading the rows it deletes.

Deliberately not an API endpoint: an endpoint that clears a second factor is a
second factor that anyone holding a session can clear.

Sessions go with it because the reason to run it is that the account is in
unknown hands; leaving a week-long session alive would leave the factor
bypassed regardless.

## Not covered

WebAuthn and passkeys. Stronger, and worth their own issue — TOTP is the
factor that needs no browser API work and the one every security review asks
for.

The dashboard enrolment and policy screens are `station:mac` work, tracked
separately; this page describes the API surface they build on.
