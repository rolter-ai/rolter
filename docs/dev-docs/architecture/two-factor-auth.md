# Two-factor authentication

TOTP (RFC 6238) as a second factor on local-account login (#1078). The
operator-facing guide is
[`docs/user-docs/security/two-factor-authentication.mdx`](../../user-docs/security/two-factor-authentication.mdx);
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
                 ├── yes ──► challenge (verify) ──► /auth/mfa/verify ──► session
                 └── no ──► policy requires one, and in force?
                             │
                             ├── no (or still in its grace window) ──► session
                             ├── yes, no ROLTER_KEK ──► 403 mfa_enrolment_required
                             └── yes ──► challenge (enrol) ──► /auth/mfa/enroll
                                                           ──► /auth/mfa/confirm ──► session
```

The challenge lives in its own table rather than as a flagged `sessions` row.
A challenge grants nothing, and putting it in `sessions` would mean every
future reader of that table had to remember to exclude it — a rule that holds
only until someone forgets it.

## Enrolment at sign-in

A `required_*` policy used to refuse a session to an account with no armed
factor, and the only enrolment path needed a session, so the member could not
fix it and an admin had to relax the policy or run the break-glass reset
(#1852). The login exchange now hands such an account an **enrolment
challenge** instead: a row in `mfa_challenges` with `purpose = 'enrol'`,
ten minutes to live, five codes to spend.

What makes it safe to hand out is how little it opens:

- It is not a session. `CurrentUser` and `Principal` look tokens up in
  `sessions` and `virtual_keys`, and it is in neither, so as a bearer it is
  refused everywhere.
- It is presented in the body of exactly two routes,
  `POST /api/v1/auth/mfa/enroll` (mint a pending secret) and
  `POST /api/v1/auth/mfa/confirm` (prove it). Every challenge read names a
  purpose, so `/auth/mfa/verify` does not find an enrolment token and the
  enrolment routes do not find a step-up token. Without that, a code from a
  secret the token minted but never armed would redeem the step-up.
- The confirm checks the code, then consumes the token with a
  `delete … returning`. Only the request whose delete removed the row goes on
  to arm the factor, mint the recovery codes and call `issue_session`, so a
  double submit cannot mint two batches, the second silently voiding the
  first.
- The session comes out of `issue_session`, the same path every other
  sign-in takes, so it is audited as `auth.login`, next to `auth.mfa_enabled`
  with `at_sign_in: true`. The password step writes `after_lock` on the
  challenge row (`mfa_challenges.after_lock`), and the redeeming request reads
  it back, so a sign-in that followed a lockout says so on its `auth.login`
  row even though a later request issued the session. The step-up carries it
  the same way.

Both writes an enrolment makes decide inside the statement rather than in a
read before it, for the same reason the replay check does:

- `MfaRepo::confirm` arms the factor only while the row still holds the
  secret the code was checked against (`secret_nonce`, fresh per seal).
  Otherwise a second live enrolment token for the same account, or a second
  tab, could replace the pending secret between the check and the arm, and the
  confirm would arm a secret its caller never saw.
- `MfaRepo::begin_enrolment` upserts only over a row whose `confirmed_at` is
  null. The `has_armed_factor` check in front of it answers the common case;
  the condition covers an enrolment racing a confirm, which would otherwise
  silently disarm the factor that confirm had just armed.

A break-glass reset, a new password set through `PUT /users/{id}` and a
deactivation (API or SCIM) all drop the user's challenges in flight. An
enrolment token is refused while a factor is armed, so without the purge, one
minted before a reset would come back to life the moment the reset cleared the
factor.

Asking for a secret does not charge the token's budget; proving one does. The
budget is not a guessing defence (whoever holds the token holds the secret)
but a bound on how long one token stays useful.

A control plane with no `ROLTER_KEK` cannot seal a secret, so it keeps the
old 403 `mfa_enrolment_required`. Handing out a challenge that fails one step
later would only move the same dead end, and the fix is the operator's. For the
same reason `PUT /orgs/{id}/auth-policy` refuses a `required_*` value there
with a 409 naming the key: without it nobody can enrol, and every account the
policy binds, the admin saving it included, would be refused at its next
sign-in.

KEK availability comes from `Kek::from_env` in production. The integration
suite installs one KEK process-wide (#1351) and cannot unset it without racing
other tests' in-flight requests, so `ControlState::mfa_without_kek`, set only
by `test_app_without_kek`, is how the tests reach these branches.

Both challenge responses carry `expires_in` next to `expires_at`, and the
dashboard times its prompts from that. Comparing `expires_at` with the browser
clock expired the prompt on arrival for anyone whose laptop clock ran more
than the TTL fast, and under `required_*` that meant never getting in.

The trade is the one every enrol-at-sign-in flow makes: until a member
enrols, their password alone gets whoever holds it through enrolment. That is
no worse than the policy being off, the grace window below gives members a
way to enrol from their own session first, and every enrolment at sign-in is
audited. A factor armed by someone else is cleared with break-glass plus a new
password; the reset alone leaves the password with whoever used it.

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

The visible consequence is that the code used to _confirm_ an enrolment is
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

| Table                                 | Holds                                                                                             |
| ------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `user_totp_factors`                   | one row per user: the sealed secret, `confirmed_at`, `last_used_step`                             |
| `user_recovery_codes`                 | hashed single-use codes; `used_at` marks a spent one                                              |
| `mfa_challenges`                      | logins in flight; hashed token, `purpose` (`verify`/`enrol`), attempt count, expiry, `after_lock` |
| `org_auth_policies.mfa_policy`        | enforcement, alongside the password/SSO switches                                                  |
| `org_auth_policies.mfa_enforce_after` | the grace window before a `required_*` policy applies (`0076`)                                    |

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

Under a `required_*` policy the password sign-in never admits an unenrolled
account unprotected: it gets the enrolment challenge above, or, on a control
plane with no KEK, a distinct `mfa_enrolment_required` refusal. Telling that
user to retype their password would be a lie. Invitation acceptance makes the
same decision for the account it creates: under an enforced policy it answers
`sign_in_required` with reason `second_factor` instead of a session, and the
sign-in that follows issues the enrolment challenge. It never mints a session
for an account that existed before the invitation (#1935; see
[invitations](invitations.md)).

`mfa_enforce_after` lets an org announce the requirement before it applies.
`SetPolicy` reads it as a double option: an explicit `null` clears it, an
absent key keeps the stored value, so a client that predates the field cannot
cancel an announced window by re-sending `mfa_policy`.
`EffectivePolicy` in `mfa.rs` reduces every org policy that binds the user:
strictest wins for the policy, and the **earliest** start wins for the window,
so one hardened org already enforcing is enough and a later window elsewhere
cannot postpone it. While every binding requirement is still in its window
the password alone signs in, and the session carries `mfa_enrol_by` so the
dashboard can say the date on the way in. `required` stays true through the
window, so removing an armed factor stays refused; the window lets the
unenrolled in for a while and never lets the enrolled back out.

Unlike `allow_password_login`, `required_all` does not exempt superadmins. The
exemption there exists because a broken IdP has no other way back in; here the
way back in is `rolter mfa reset`, which needs no exemption to work.

## Break-glass

`rolter mfa reset --email ... --reason ...` clears the factor and its recovery
codes, revokes every live session for the account, drops its challenges in
flight, and writes an audit entry carrying the reason. It runs on the host
against the database, which is the same privilege level as reading the rows it
deletes.

Deliberately not an API endpoint: an endpoint that clears a second factor is a
second factor that anyone holding a session can clear.

Sessions go with it because the reason to run it is that the account is in
unknown hands; leaving a week-long session alive would leave the factor
bypassed regardless.

## Not covered

WebAuthn and passkeys. Stronger, and worth their own issue — TOTP is the
factor that needs no browser API work and the one every security review asks
for.

The dashboard builds on this surface: `ui/src/components/SignInEnrolment.tsx`
is the enrolment step in the sign-in card, loaded lazily so the QR encoder
stays out of the chunk every signed-out visitor downloads, and the grace
window is the **Start requiring it** control on the SSO screen's sign-in
policy card.
