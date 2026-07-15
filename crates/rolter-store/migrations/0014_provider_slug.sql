-- stable, URL-safe provider slug for `provider-slug/model` addressing
-- (ADR: provider/model addressing, ROL-266/ROL-269). The slug is a first-class
-- identity: unique per org, immutable by default (renaming the display name
-- never changes it), charset `^[a-z0-9][a-z0-9-]{0,62}$`.

alter table providers add column if not exists slug text;

-- backfill existing rows from a slugified name with numeric de-dup on
-- collision within an org. non-alphanumerics collapse to single hyphens,
-- leading/trailing hyphens trim, and a name with no ascii alphanumerics falls
-- back to 'provider'. the first row for a base keeps the bare slug; later ones
-- get a '-N' suffix (base truncated to leave room for the suffix).
with slugged as (
    select
        id,
        org_id,
        coalesce(
            nullif(trim(both '-' from regexp_replace(lower(name), '[^a-z0-9]+', '-', 'g')), ''),
            'provider'
        ) as base
    from providers
    where slug is null
),
numbered as (
    select
        id,
        base,
        row_number() over (partition by org_id, base order by id) as rn
    from slugged
)
update providers p
set slug = case
        when n.rn = 1 then left(n.base, 63)
        else left(n.base, 58) || '-' || n.rn::text
    end
from numbered n
where p.id = n.id;

alter table providers alter column slug set not null;

alter table providers
    add constraint providers_org_slug_unique unique (org_id, slug);

alter table providers
    add constraint providers_slug_charset
    check (slug ~ '^[a-z0-9][a-z0-9-]{0,62}$');
