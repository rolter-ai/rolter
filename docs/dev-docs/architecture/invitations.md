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

One live invitation exists per email per org, enforced by a partial unique
index. Re-inviting replaces rather than accumulates.

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
