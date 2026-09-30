-- #1823: a self-service profile on the account. `display_name` and `bio` are
-- optional free text the signed-in user edits through `PATCH /api/v1/me/profile`
-- at any role, so the dashboard can show a person instead of an email address.
--
-- the bounds live here as well as in the API so a bulk import, a SCIM sync or a
-- hand-written update cannot store something the dashboard was never written
-- to render. `~` with `[[:cntrl:]]` rejects control characters (newlines are
-- allowed in a bio only, hence the narrower class there).
--
-- the data plane does not read these columns: the snapshot reads only
-- `deactivated_at` and `is_superadmin` from `users`, and the
-- `users_bump_config_version` trigger (0075) is `after update of` exactly those
-- two. a profile edit therefore must not, and does not, bump `config_version`,
-- so no new trigger is needed.

alter table users
    add column display_name text,
    add column bio text;

alter table users
    add constraint users_display_name_shape check (
        display_name is null
        or (char_length(display_name) between 1 and 80
            and display_name = btrim(display_name)
            and display_name !~ '[[:cntrl:]]')
    ),
    add constraint users_bio_shape check (
        bio is null
        or (char_length(bio) between 1 and 500
            and bio = btrim(bio)
            and bio !~ '[\x01-\x08\x0b\x0c\x0e-\x1f\x7f]')
    );
