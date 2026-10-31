-- where a model price came from (#966). a row an operator wrote through
-- `PUT /api/v1/model-prices` (or the seed import) has no source: it is a
-- deliberate statement and an override, and a catalogue import never replaces
-- it. a row the catalogue importer wrote names the catalogue in `source` and
-- the day its snapshot was taken in `source_imported_at`, so the dashboard can
-- say "catalogue value, subject to change" and trace a wrong number to its
-- origin. the importer is source-agnostic, so `source` is free text rather than
-- an enum.
--
-- no new trigger: `model_prices_bump_config_version` (0004) already fires on
-- every insert, update and delete, and these columns change with the row.
alter table model_prices
    add column if not exists source text,
    add column if not exists source_imported_at timestamptz;
