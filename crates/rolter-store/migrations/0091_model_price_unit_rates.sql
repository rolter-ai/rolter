-- #2937: rates for the requests a provider bills by something other than
-- tokens.
--
-- an image request is billed per image, a transcription per minute of audio
-- and a text-to-speech request per character of input. cost was computed from
-- token counts alone, so such a request on a model with no token price was
-- recorded `unpriced = 1` with no way to price it. the three columns below are
-- those rates, in the row's `currency`:
--
--   image_per_unit          one generated image
--   audio_input_per_minute  one minute of audio sent to a transcription or
--                           translation model
--   speech_per_mchar        one million characters of text sent to a
--                           text-to-speech model
--
-- null means the row sets no such rate, which is every existing row, so a
-- request is charged exactly what it was before this migration. a unit rate
-- is added to the token cost of the same request, never substituted for it.
--
-- no new trigger: `model_prices_bump_config_version` (0004) already fires on
-- every insert, update and delete of the table, which is how a write to these
-- columns reaches /internal/snapshot and the gateway's next reload.
--
-- held to the rule 0084 and 0085 set for the token rates: at least zero and
-- not NaN, since a negative rate would turn a request into a credit against a
-- budget and `numeric` sorts NaN above every number. the columns are new, so
-- every row is null (a check passes on null) and the constraints validate as
-- they are added.

alter table model_prices
    add column if not exists image_per_unit numeric(12, 6),
    add column if not exists audio_input_per_minute numeric(12, 6),
    add column if not exists speech_per_mchar numeric(12, 6),
    add constraint model_prices_image_per_unit_rate
        check (image_per_unit >= 0 and image_per_unit <> 'NaN'::numeric),
    add constraint model_prices_audio_input_per_minute_rate
        check (audio_input_per_minute >= 0 and audio_input_per_minute <> 'NaN'::numeric),
    add constraint model_prices_speech_per_mchar_rate
        check (speech_per_mchar >= 0 and speech_per_mchar <> 'NaN'::numeric);
