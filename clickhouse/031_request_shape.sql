-- What a request asked the model to do beyond its token counts (#3317).
--
-- The savings advisor (#1581) lists requests that could have gone to a cheaper
-- model in the same route. A request that offered tools, sent an image or
-- audio, carried several caller turns or asked for a structured-output schema
-- needs a model that does those, so the advisor excludes it under its own
-- reason (`uses_tools`, `image_input`, `audio_input`, `structured_output`,
-- `multi_turn`). The gateway reads the flags off the request body it already
-- parsed, for the OpenAI (Chat Completions, Responses) and Anthropic (Messages)
-- dialects:
--
--   has_tools            1 when the request offered tools
--   has_image_input      1 when an input message carried an image
--   has_audio_input      1 when an input message carried audio
--   has_response_schema  1 when it asked for output following a JSON schema
--   turns                the caller turns it carried (user messages or items)
--
-- Every column is 0 for a request that is not a chat-shaped model call (an
-- embedding, a rerank, a decision) and for every row older than this file.
--
-- A gateway inserts with `input_format_skip_unknown_fields=1` and leaves each
-- field out of a row while it is zero, so one upgraded before this is applied
-- keeps logging and loses only the flags. Applying this file is not a
-- precondition of upgrading the gateway or the control plane; apply it so the
-- advisor can judge a request on its shape.

alter table request_logs
    add column if not exists has_tools UInt8 default 0,
    add column if not exists has_image_input UInt8 default 0,
    add column if not exists has_audio_input UInt8 default 0,
    add column if not exists has_response_schema UInt8 default 0,
    add column if not exists turns UInt16 default 0;
