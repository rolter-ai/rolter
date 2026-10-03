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
   shown exactly once**; only its peppered SHA-256 digest is stored, the same
   treatment sessions and virtual keys get, so a database dump alone yields no
   usable link.
2. `GET /api/v1/invitations/accept/{token}` — unauthenticated preview returning
   the org name, the invited email, the role, the expiry, and `has_account`
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
`invited_by` records the caller, and is `null` for the admin token.

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
([#2325](https://github.com/rolter-ai/rolter/issues/2325)).

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

rolter does not send the email itself: it returns the link and lets the operator
deliver it however their organization already delivers things. That keeps the
control plane free of an SMTP dependency, which matters for
[air-gapped](../deployment/air-gapped.md) deployments.

## Related

- [Single sign-on (OIDC)](sso.md) — the IdP path, and how membership sources
  keep the two from fighting
- [RBAC & auth](rbac-and-auth.md) — roles and scopes
- [SCIM provisioning](scim-provisioning.md) — IdP-driven lifecycle for
  deployments that want accounts created without anyone clicking a link
