-- enrolling a second factor at sign-in, and an org grace window before a
-- `required_*` policy starts to bite (#1852).
--
-- Like 0067, none of this carries a `bump_config_version()` trigger and none
-- may grow one: the data plane never reads it. The factor is checked once, on
-- the control plane's login exchange, so there is nothing for
-- `/internal/snapshot` to propagate.

-- what a challenge may be redeemed for. `verify` is the step-up 0067 added: a
-- login that owes a code from a factor already armed. `enrol` is new: a login
-- bound by a `required_*` policy with no factor armed, which may do nothing but
-- mint a secret and prove it. Kept in the same table because both are the same
-- thing -- a hashed, short-lived token naming a login in flight that
-- authenticates nothing -- and a column is what stops one being presented where
-- the other is expected. Existing rows are all step-ups, hence the default.
alter table mfa_challenges
    add column if not exists purpose text not null default 'verify';

do $$
begin
    alter table mfa_challenges
        add constraint mfa_challenges_purpose_check
        check (purpose in ('verify', 'enrol'));
exception
    when duplicate_object then null;
end
$$;

-- whether the password step that minted the challenge came straight after a
-- lockout. The session is issued by a later request, the one that redeems the
-- challenge, and that request no longer knows; without this the `auth.login`
-- row an investigator reads after a credential-stuffing run looks like any
-- other sign-in. Existing rows predate the column and are recorded as not.
alter table mfa_challenges
    add column if not exists after_lock boolean not null default false;

-- the moment a `required_*` policy starts sending unenrolled members through
-- enrolment before they get a session. null means it already does. A date
-- lets an org announce the requirement and give people time to enrol from
-- their account first; until it passes, an unenrolled member signs in with
-- the password alone, exactly as under `optional`.
alter table org_auth_policies
    add column if not exists mfa_enforce_after timestamptz;
