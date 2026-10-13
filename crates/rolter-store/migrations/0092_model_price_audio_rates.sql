-- #1882: audio token rates on a model price.
--
-- a realtime or audio model bills the audio tokens it is sent and returns at a
-- different rate from the text ones, and says how many of each in its usage
-- (`input_token_details.audio_tokens`, `output_token_details.audio_tokens`,
-- and `cached_tokens_details.audio_tokens` for the cache hits). a row had one
-- input and one output rate, so every audio token was charged as text. the
-- three columns below are those rates, per million tokens, in the row's
-- `currency`:
--
--   audio_input_per_mtok         audio tokens in the prompt
--   audio_output_per_mtok        audio tokens in the completion
--   cached_audio_input_per_mtok  audio tokens among the prompt's cache hits
--
-- null means the row sets no such rate and the audio token is charged at the
-- matching text rate, which is every existing row, so a request is charged
-- exactly what it was before this migration.
--
-- no new trigger: `model_prices_bump_config_version` (0004) already fires on
-- every insert, update and delete of the table, which is how a write to these
-- columns reaches /internal/snapshot and the gateway's next reload.
--
-- held to the rule 0084, 0085 and 0091 set for the other rates: at least zero
-- and not NaN. the columns are new, so every row is null (a check passes on
-- null) and the constraints validate as they are added.

alter table model_prices
    add column if not exists audio_input_per_mtok numeric(12, 6),
    add column if not exists audio_output_per_mtok numeric(12, 6),
    add column if not exists cached_audio_input_per_mtok numeric(12, 6),
    add constraint model_prices_audio_input_per_mtok_rate
        check (audio_input_per_mtok >= 0 and audio_input_per_mtok <> 'NaN'::numeric),
    add constraint model_prices_audio_output_per_mtok_rate
        check (audio_output_per_mtok >= 0 and audio_output_per_mtok <> 'NaN'::numeric),
    add constraint model_prices_cached_audio_input_per_mtok_rate
        check (cached_audio_input_per_mtok >= 0 and cached_audio_input_per_mtok <> 'NaN'::numeric);
