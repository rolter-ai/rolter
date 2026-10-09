-- #2889: a price rate is a real number of at least zero.
--
-- `PUT /api/v1/model-prices` used to accept any text that parses as a float,
-- which includes a negative rate (the request then credits its budget instead
-- of spending it) and `NaN` (which `numeric` stores, and which the snapshot
-- loader cannot read back). the API refuses both now; this keeps a row written
-- some other way, or by an older control plane during a rolling upgrade, out
-- of the table too.
--
-- `numeric` sorts NaN above every number, so `>= 0` alone lets it through:
-- 0083's check on `cache_write_per_mtok` did, and is replaced here by the same
-- rule as the other three. infinity cannot be stored in `numeric(12, 6)`, so
-- there is nothing to rule out there.
--
-- a migration must not brick a database that already holds a bad row, which
-- the old API could have written. every constraint is therefore added `not
-- valid`: it checks each insert and update from now on and leaves existing rows
-- alone. the block below then validates each one that can be, so the usual
-- database, with nothing wrong in it, ends up with fully validated constraints.
-- a constraint whose column holds a bad row stays `not valid` until the
-- operator corrects the row and runs `alter table model_prices validate
-- constraint <name>`, which docs/user-docs/deployment/upgrading.mdx walks
-- through. until then the control plane leaves that row out of the snapshot
-- with a problem line rather than pricing the model at zero.
--
-- no new trigger: `model_prices_bump_config_version` (0004) already fires on
-- every write to the table, and a constraint changes no row.

alter table model_prices
    drop constraint if exists model_prices_cache_write_per_mtok_check,
    add constraint model_prices_input_per_mtok_rate
        check (input_per_mtok >= 0 and input_per_mtok <> 'NaN'::numeric) not valid,
    add constraint model_prices_output_per_mtok_rate
        check (output_per_mtok >= 0 and output_per_mtok <> 'NaN'::numeric) not valid,
    -- the optional rates are null when absent, and a check passes on null
    add constraint model_prices_cached_input_per_mtok_rate
        check (cached_input_per_mtok >= 0 and cached_input_per_mtok <> 'NaN'::numeric) not valid,
    add constraint model_prices_cache_write_per_mtok_rate
        check (cache_write_per_mtok >= 0 and cache_write_per_mtok <> 'NaN'::numeric) not valid;

do $$
declare
    rate_check text;
begin
    foreach rate_check in array array[
        'model_prices_input_per_mtok_rate',
        'model_prices_output_per_mtok_rate',
        'model_prices_cached_input_per_mtok_rate',
        'model_prices_cache_write_per_mtok_rate'
    ] loop
        begin
            execute format('alter table model_prices validate constraint %I', rate_check);
        exception when check_violation then
            raise notice 'model_prices constraint % holds a row that breaks it and stays not valid; see Upgrading in the Rolter docs',
                rate_check;
        end;
    end loop;
end
$$;
