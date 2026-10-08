-- Which call on a stored response a request-log row records (#2836).
--
-- `GET`, `DELETE`, `cancel` and `input_items` on `/v1/responses/{id}` now leave
-- a row like every other request from an identified caller. Nothing else on
-- such a row tells them apart: they share a model and a provider, carry no
-- tokens, cost nothing, and a `GET` and a `DELETE` of the same response differ
-- only in what they did to it.
--
-- `lifecycle_operation` is `retrieve`, `delete`, `cancel` or `input_items`,
-- or `compact` / `input_tokens` for the two calls the gateway answers with
-- `501`. It is empty for a request that ran a model, which is every row older
-- than the column.
--
-- A gateway inserts with `input_format_skip_unknown_fields=1`, so one upgraded
-- before this is applied keeps logging and leaves the field out of its rows.

alter table request_logs
    add column if not exists lifecycle_operation String default '';
