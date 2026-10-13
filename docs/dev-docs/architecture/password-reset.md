# Administrator password reset

An administrator can reset a member's password (#2240) without ever knowing it.
The mechanism is the [invitation](invitations.md) one applied to an existing
local account: a one-time link the member redeems to choose their own password.

## Why a link and not a temporary password

A temporary password is a credential the administrator reads and relays, usually
through a chat log. The link carries no password, only the right to set one, and
only its peppered digest is stored (`password_resets.token_hash`, the same
treatment as invitations and sessions).

## Issuing

`POST /api/v1/orgs/{org_id}/users/{id}/password-reset` (`password_reset:create`,
admin at the org; `password_reset.rs`):

- below a superadmin the account must belong to the org alone and must not be a
  superadmin. A link lets its holder choose the password of the whole account,
  which spans every org it belongs to; letting an org admin take over an account
  that is also a member elsewhere, or a superadmin, would be an escalation;
- refused for the caller's own account, an account with no local password
  (single sign-on and LDAP accounts sign in elsewhere; giving one a password adds
  a way in the identity provider does not control) and a deactivated account;
- `PasswordResetRepo::issue` revokes the account's live reset and inserts the
  new one under an advisory lock, so only the newest link works
  (`password_resets_live_user_idx`);
- deletes the member's sessions and MFA challenges at once;
- leaves `users.password_hash` untouched, so a mistaken click locks nobody out.
  The lever for a known-compromised password is deactivation;
- audits `user.password_reset` in the org with the issuer, the sessions ended
  and the replaced reset. The token is never logged.

## Redeeming

`GET`/`POST /api/v1/password-reset/{token}` are on the public-routes allowlist;
the token is the credential and both go through the login throttle keyed on the
submitted token and the client address. `PasswordResetRepo::redeem` stamps the
reset used, stores the new hash and deletes every session in one transaction, so
a second redeem, or one racing the first, loses. A deactivated account rolls the
whole thing back.

Redeeming **signs nobody in**, as accepting an invitation for an existing account
does (#1935): the member goes to the normal sign-in, so an armed TOTP factor
still applies and a link held by someone else cannot get past it. Nothing here
clears a factor; `rolter mfa reset` remains the host-side break-glass.

## Storage

`password_resets` (migration `0089`) is read by the control plane only, so it has
no `bump_config_version()` trigger. Rows go with the account (`on delete
cascade`); `created_by` is nulled when the issuing account is deleted.

## Dashboard

The Users screen has a key button per active row (`password_reset:create` gate)
that confirms and then reveals the link in the shared one-time reveal dialog. The
member lands on `/reset-password/{token}`, a public page outside the signed-in
shell that previews the address and posts the new password.
