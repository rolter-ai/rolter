-- #3459: one `slug/model` address per org across providers and provider groups.
--
-- Providers and provider groups share one slug namespace per org at the
-- gateway (`provider-slug/model`, `group-slug/model`; #1857, ADR-0040). Each
-- table is unique on `(org_id, slug)`, but nothing held the two tables apart:
-- the control plane checked the other table and then inserted, so a provider
-- and a group created concurrently in one org could both take a slug, and the
-- second holder silently took the address from the first.
--
-- A unique constraint cannot span two tables, so a trigger holds the rule. It
-- serialises every write of one `(org, slug)` on a transaction-scoped advisory
-- lock and then looks in the other table; plpgsql takes a fresh snapshot per
-- statement under read committed, so the second writer sees the first one's
-- committed row once it has the lock. The refusal is a unique violation, which
-- the API already answers with `409 name_taken`.
--
-- Rows that already share a slug are left as they are: only a write that
-- changes a row's org or slug is checked.

create or replace function check_address_slug_free() returns trigger
language plpgsql as $$
begin
    if tg_op = 'UPDATE' and new.slug = old.slug and new.org_id = old.org_id then
        return new;
    end if;
    perform pg_advisory_xact_lock(
        hashtextextended('rolter.address_slug:' || new.org_id::text || ':' || new.slug, 0));
    if tg_table_name = 'providers' then
        if exists (select 1 from provider_groups
                    where org_id = new.org_id and slug = new.slug) then
            raise exception 'slug % is already a provider group address in org %',
                new.slug, new.org_id
                using errcode = '23505';
        end if;
    elsif exists (select 1 from providers
                   where org_id = new.org_id and slug = new.slug) then
        raise exception 'slug % is already a provider address in org %',
            new.slug, new.org_id
            using errcode = '23505';
    end if;
    return new;
end;
$$;

drop trigger if exists providers_address_slug_free on providers;
create trigger providers_address_slug_free
    before insert or update of slug, org_id on providers
    for each row execute function check_address_slug_free();

drop trigger if exists provider_groups_address_slug_free on provider_groups;
create trigger provider_groups_address_slug_free
    before insert or update of slug, org_id on provider_groups
    for each row execute function check_address_slug_free();

-- the data plane reads both tables, and their `*_bump_config_version` statement
-- triggers (0003, 0071) fire on every write with no column list. Nothing to add.
