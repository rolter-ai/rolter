# Invitations

Invitations are how a rolter deployment onboards people **without an identity
provider**. An admin mints a one-time link, the invitee opens it and chooses
their own password, and the role the invitation carries is granted on
acceptance.

This is deliberately not the same as `POST /api/v1/orgs/{org_id}/users`, which
creates an account with a password the admin picked. That is right for seeding
and for service accounts, and wrong for onboarding a colleague: it leaves
someone else holding their credential — usually in a chat log.

## The flow

1. `POST /api/v1/orgs/{org_id}/invitations` (admin at the target scope) returns
   the invitation, the token, and a ready-made `accept_url`. **The token is
   shown exactly once**; what sign-in reads is its peppered SHA-256 digest, the
   same treatment sessions and virtual keys get. With a mail relay and a KEK the
   token is also kept sealed with the KEK (`token_ciphertext`, see the resend
   below), so a dump alone yields no usable link but a dump with the KEK does,
   for as long as the invitation is live. That copy is not tied to the pepper:
   rotating `ROLTER_SESSION_PEPPER` stops the digest matching, so the link no
   longer signs anyone in, yet a resend still opens the sealed token and mails a
   link that is dead. Revoke and re-invite after a pepper change.
2. `GET /api/v1/invitations/accept/{token}` — unauthenticated preview returning
   the org name, the invited email, the role, the scope it applies at
   (`scope_type` of `org`, `team` or `project`, plus the team or project's
   `scope_name`), the expiry, and `has_account`
   (whether an account already exists under that email, so the accept screen
   knows whether to ask for a password). Nothing else: a link that leaked
   should not also leak a directory.
3. `POST /api/v1/invitations/accept/{token}/accept` grants the membership. For
   an email with no account yet it takes the invitee's chosen `password`,
   creates the account and returns a live session so they land signed in,
   unless the org requires a second factor (below). For an existing account it
   takes no password and returns no session (see
   [Existing accounts](#existing-accounts)).
4. `DELETE /api/v1/invitations/{id}` revokes a pending invitation.

Links expire after seven days.

### Failing closed

Expired, revoked, already-accepted and simply wrong tokens all return the same
`401`. The caller learns whether their link works, not which of those it is.

Acceptance is single-use even under a race: the claim is an
`UPDATE … WHERE accepted_at IS NULL`, which either affects one row or tells the
loser they lost — before any account or membership is created.

One unaccepted, unrevoked invitation exists per email per org, enforced by a
partial unique index (`invitations_live_email_idx`). The index does not look at
`expires_at`, because a partial-index predicate cannot use `now()`, so an
invitation that expired unaccepted would still hold its address. The create path
therefore handles both cases.

### Re-inviting replaces

Inviting an address that already holds an unaccepted, unrevoked invitation in
the same org, expired or not, **replaces** it
([#2324](https://github.com/rolter-ai/rolter/issues/2324)).
`InvitationRepo::create` runs in one transaction that sets `revoked_at = now()`
on the old row (the same thing `DELETE /api/v1/invitations/{id}` does) and
inserts the new one. The new link is the only live one and the old link is
refused like any revoked link (`401`). The email match is case-insensitive,
like the index. An accepted invitation never counts, so the address can be
invited again.

Concurrent creates for one address are ordered by a transaction-scoped advisory
lock on `(org_id, lower(email))`, taken before the revoke. The second request
waits, revokes the first one's fresh row and inserts its own, so both answer
`200` and exactly one live row remains. Because the lock serializes the
revoke-then-insert, the unique index cannot be hit by two creates for one
address.

The audit log records the replacement on the new invitation's `invitation.create`
entry as `replaced: <old invitation id>` (`null` when nothing was replaced). No
separate `invitation.revoke` entry is written for it. The dashboard lists
expired invitations so they can be revoked by hand; replacing does it
implicitly.

## Who may invite, list and revoke

Creating is authorized at the scope the invitation grants: `scope_type` and
`scope_id` name an org, a team or a project, and the caller needs `invitation`
create there or above, the same admin bar as granting a role directly.
`invited_by` records the signed-in account behind the request, superadmin or
not, and is `null` only when no account is signed in: the admin token, or open
mode. A superadmin session is not a `Principal::User` (`Principal::for_user`
turns it into `Principal::Superadmin`, and the operator `rolter-seed` creates
holds no membership at all), so `Principal::Superadmin` carries the account it
came from and `Principal::account_id()` is the one way to read it
([#2813](https://github.com/rolter-ai/rolter/issues/2813)). Read the sender from
`account_id()`, never by matching `Principal::User` alone, or a superadmin's
records go missing.

`GET /api/v1/orgs/{org_id}/invitations` answers every invitation of the org,
accepted and revoked ones included. An org admin gets all of them; below the org
the answer is filtered to the teams and projects the caller administers (#1850),
and a caller who reaches the org through neither gets `403`. Revoking is
authorized at the invitation's own scope, and only touches an unaccepted
invitation: an accepted one answers `404`.

## The dashboard

**Governance → Users → Invite user** (`ui/src/pages/Users.tsx`) picks the scope
with the shared `OrgScopePicker` and posts `scope_type` and `scope_id`. The
role field is named for the scope (`pages.users.orgRole`, `teamRole`,
`projectRole`) and its options are labelled through `shell.roles.*`. The
password method creates the account through `POST /orgs/{org_id}/users`, which
has no scope, so the picker is locked to the org while it is chosen. The link
dialog is the shared `SecretRevealDialog`: a clipboard the browser withholds
(a plain-http dashboard) leaves a failed-copy message under the link, which
stays on screen and selected, and closing the dialog before the link was copied
asks first. See [Dashboard one-time secrets](../development/secret-reveal.md).

`PendingInvitations`, below the users table, keeps the rows with neither
`accepted_at` nor `revoked_at`, including expired ones (marked), and revokes
through `ConfirmDialog` (`invitation-revoke`), gated on `invitation:delete`. The
section waits for the capability answer and is absent on an explicit
`invitation:read` refusal or a `403` from the list, so a viewer who may open the
screen never meets a `forbidden` error for it. "Sent by" is the users-list entry
for `invited_by`, since the row carries the id only
([#2325](https://github.com/rolter-ai/rolter/issues/2325)). A superadmin's own
list is read with `include_unassigned=true`, so a sender who holds no role
anywhere is in it; an org admin's list is not, and shows such a sender as
"unknown" rather than leaking an account outside their organization.

The accept screen (`ui/src/pages/AcceptInvite.tsx`) names the role through
`roleLabel`, the same `shell.roles.*` keys the invite dialog and the pending
list use, so the invitee reads "Member" as the inviter did and not the stored id
`member`. A role the catalog has no key for is shown as the control plane sent it
([#2813](https://github.com/rolter-ai/rolter/issues/2813)).

## Existing accounts

Acceptance adopts an account that already exists under the invited email rather
than forking a second row for the same person. Someone may already hold a login
in another org, or have arrived through SSO first.

The token proves that someone was sent the link, not who holds it: whoever
created the invitation got the same token back. So an invitation never signs
anyone in to an account that existed before it (#1935). Otherwise an org admin
could invite a superadmin's email, accept the link themselves, and walk away
with that superadmin's session.

- Accepting grants the invited role and answers
  `{"sign_in_required": true, "reason": "existing_account", "email": …}` with
  no session. The invitee signs in through `POST /api/v1/auth/login` as usual,
  so their own password and any second factor still apply.
- No credential changes. An account with a password keeps it, and an SSO-only
  account (no password) stays SSO-only: a `password` in the body is ignored.
  An invite link is neither a sign-in nor a password reset; a superadmin sets a
  password through `PUT /api/v1/users/{id}` when one is really wanted.
- A deactivated account cannot be revived by an invitation (`403`).

## Second-factor policy

A new account created by an invitation goes through the decision a password
sign-in makes ([two-factor auth](two-factor-auth.md)), once the membership that
binds it to the org's policy exists. When a `required_*` policy is in force the
answer is `{"sign_in_required": true, "reason": "second_factor", …}` and no
session: the invitee signs in with the password they just chose, and that
sign-in issues the enrolment challenge. While the policy is only announced, the
session goes through with `mfa_enrol_by`, as on a sign-in.

A missing or too-short password for a new account is refused with `400` before
the invitation is claimed, so the link stays usable.

## Naming the sender

`GET /api/v1/orgs/{org_id}/invitations` returns each row with `invited_by` (the
sender's account id, null for the admin token or a deleted sender) and
`invited_by_email`, resolved in the same query by a left join on `users`
(`InvitationRepo::list`). The address does not depend on which users the caller
may list: a team admin's users list holds only their own teams, but the
invitation row is already visible to them, so the sender is too. The field is
set on the list only; `create` and `revoke` return it null.

## Relationship to single sign-on

Invitations and [single sign-on](sso.md) co-exist; neither requires the other.

The membership an acceptance grants carries `source = 'manual'`, and an SSO
login only ever reconciles `source = 'sso'` rows. So an invited role survives
every later IdP login, while roles that came from IdP groups are recomputed
each time. A deployment can run invitations only, SSO only, or both at once.

## Configuration

| Setting                 | Where | Notes                                                                                                                                                              |
| ----------------------- | ----- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `ROLTER_PUBLIC_URL`     | env   | the `accept_url` is built from it; set it correctly behind a proxy or the link you hand out points at localhost. Read once at startup, so a change needs a restart |
| `ROLTER_SESSION_PEPPER` | env   | peppers the stored token digest, same as session tokens                                                                                                            |
| `ROLTER_SMTP_URL`       | env   | the mail relay (`smtp://` or `smtps://`, credentials in the URL). Optional; unset means invitations are not emailed                                                |
| `ROLTER_SMTP_FROM`      | env   | the sender mailbox. Required with `ROLTER_SMTP_URL`; without it mail stays off and the control plane logs why                                                      |

## Email delivery

With `ROLTER_SMTP_URL` set, `create_invitation` also sends the link to the
invitee (#1828, `crates/rolter-control/src/mail.rs`). The link is **still
returned** to the admin, so the email is a convenience over the fallback and not
a replacement for it: a relay that is down, or a typo in the address, never
fails an invitation.

- The outcome is kept on the row: `mail_status` is `not_configured` (no
  transport when it was created, and every invitation that predates
  migration `0105`), `sent` or `failed`, with `mail_attempted_at` and, for a
  failure, a `mail_error` written to be safe to show an admin ("the mail server
  rejected the message"). The relay's own reply is never stored: it often names
  the recipient.
- Each delivery is audited as `invitation.mail_sent` or `invitation.mail_failed`
  on the invitation. Neither the address nor the link is in the audit detail or
  in any log line, and neither is the SMTP URL, which can carry a password.
- `POST /api/v1/invitations/{id}/resend` (#3304) mails the same link again. Only
  a digest of the token is kept for sign-in, so with a relay and a KEK
  `create_invitation` also stores the token sealed (`token_ciphertext` and
  `token_nonce`, migration `0155`, listed in `SEALED_COLUMNS`); a resend opens
  it, keeps the expiry and records the outcome on the row. Accepting, revoking
  or replacing an invitation clears the sealed copy, and so does expiry: the
  background `expiry_sweep` (every five minutes) calls
  `InvitationRepo::sweep_expired_sealed_tokens` for each invitation past
  `expires_at`, since an expired link opens nothing and the copy would otherwise
  live as long as the row (#3397). `sealed_token` refuses an expired row itself,
  so one waiting for the sweep is inert. One without it (created
  before `0155`, without a relay, or without a KEK) answers `409` and has to be
  revoked and sent again. It is guarded as creating the invitation is, at the
  invitation's own scope, and audits `invitation.resend` followed by the usual
  `invitation.mail_sent` or `invitation.mail_failed`.
- `POST /api/v1/orgs/{org_id}/invitations/mail-check` and the `--connect`
  mode of `rolter check` run `Mailer::check`: connect, TLS and authentication
  against the relay, no message. A failure names its stage (`connect`, `tls`,
  `auth`, `relay`) with a fixed sentence; the relay's reply, its address and the
  credential never leave the process. Every run dials the relay, so `CallGap`
  (`call_gap.rs`, held in `ControlState::mail_check_gap`) lets a caller (the
  `caller_key` the CSV export counts against) run one every five seconds (and anyone one every two, the relay being one shared resource) and
  answers `ApiError::TooManyAttempts` (`429` with `Retry-After`, #3394) in
  between. It is process-local; the integration suite's app shortens the gap.
- The send is awaited, bounded by a 15 second timeout, so the response says what
  happened. `Mailer` (`mail.rs`) is the seam the integration suite installs to
  record messages and to fail on demand.
- The relay address is checked against the egress policy at startup
  (`EgressPolicy::url_deny_reason`), and certificates are verified through the
  platform trust store like the upstream client, on the same aws-lc-rs rustls
  provider as everything else. A configuration that does not work is logged once
  and turns mail off, not the control plane.
- The message is plain text: who it is from, the role, the link and when it
  expires. The organization name has its control characters removed before it
  reaches the subject.

Without a transport the behaviour is what it always was, and the dashboard says
so next to the row. An air-gapped deployment configures nothing.

## Related

- [Single sign-on (OIDC)](sso.md) — the IdP path, and how membership sources
  keep the two from fighting
- [RBAC & auth](rbac-and-auth.md) — roles and scopes
- [SCIM provisioning](scim-provisioning.md) — IdP-driven lifecycle for
  deployments that want accounts created without anyone clicking a link
