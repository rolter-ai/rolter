-- TOTP second factor for local accounts, plus the per-org enforcement policy
-- (#1078).
--
-- Like `custom_roles` in 0058, none of this carries a `bump_config_version()`
-- trigger and none may grow one: the data plane never reads it. A second
-- factor is checked once, on the control plane's login exchange, and the
-- session it issues is what every later request presents -- so there is
-- nothing for `/internal/snapshot` to propagate, and bumping the version on
-- every enrolment would wake the whole gateway fleet for a change it cannot
-- observe.

-- one factor per user. the shared secret is sealed with the deployment KEK,
-- the same way `provider_keys.ciphertext` and `sso_providers.secret_ciphertext`
-- are: a TOTP secret is a bearer credential -- anyone holding it mints valid
-- codes forever -- so a database dump alone must not yield one. It is
-- registered in `SEALED_COLUMNS` (`kek_audit.rs`) so `rolter kek verify`
-- reports a restore that cannot open it.
create table if not exists user_totp_factors (
    user_id          uuid primary key references users (id) on delete cascade,
    secret_ciphertext bytea not null,
    secret_nonce      bytea not null,
    -- null until the user has proved they can generate a code from the secret.
    -- an unconfirmed row is an enrolment in progress and grants nothing: it
    -- must never be treated as an armed factor, or a half-finished enrolment
    -- would lock the account out
    confirmed_at     timestamptz,
    -- the last TOTP step this factor accepted. RFC 6238 codes stay valid for
    -- their whole step (and rolter's skew window widens that to three), so
    -- without this a shoulder-surfed code is replayable for up to 90 seconds.
    -- a verification is only accepted for a step strictly greater than this
    last_used_step   bigint,
    created_at       timestamptz not null default now(),
    updated_at       timestamptz not null default now()
);

-- single-use recovery codes, hashed the same way virtual keys and session
-- tokens are (`rolter_auth::hash_key`), never stored in the clear. They are
-- shown once at generation; a lost set is regenerated, which replaces the
-- whole batch rather than topping it up, so "how many do I have left" always
-- has one answer.
create table if not exists user_recovery_codes (
    id        uuid primary key default gen_random_uuid(),
    user_id   uuid not null references users (id) on delete cascade,
    code_hash text not null unique,
    used_at   timestamptz,
    created_at timestamptz not null default now()
);

create index if not exists idx_user_recovery_codes_user on user_recovery_codes (user_id);

-- short-lived challenges issued by the login exchange when a factor is armed.
-- deliberately NOT a row in `sessions`: a challenge authenticates nothing, and
-- putting it there would mean every reader of `sessions` had to remember to
-- exclude it. Hashed like a session token, so a dump does not yield a
-- resumable half-login.
create table if not exists mfa_challenges (
    id         uuid primary key default gen_random_uuid(),
    user_id    uuid not null references users (id) on delete cascade,
    token_hash text not null unique,
    -- failed second-factor attempts against this challenge; the challenge is
    -- spent once it is exhausted, so a stolen password plus unlimited guesses
    -- is not six digits away from a session
    attempts   integer not null default 0,
    created_at timestamptz not null default now(),
    expires_at timestamptz not null
);

create index if not exists idx_mfa_challenges_expires on mfa_challenges (expires_at);

-- per-org enforcement, alongside the existing login policy. `off` is the
-- default so no existing deployment changes behaviour on upgrade.
--
-- `required_all` deliberately does not exempt superadmins: unlike
-- `allow_password_login`, an enforced second factor has a break-glass path
-- that does not need an exemption (`rolter mfa reset`, run on the host with
-- database access), so exempting the most privileged account would weaken the
-- policy for nothing.
alter table org_auth_policies
    add column if not exists mfa_policy text not null default 'off';

do $$
begin
    alter table org_auth_policies
        add constraint org_auth_policies_mfa_policy_check
        check (mfa_policy in ('off', 'optional', 'required_superadmin', 'required_all'));
exception
    when duplicate_object then null;
end
$$;
