-- Why a request reached the target that answered it (#1584).
--
-- The class token of the failure that sent the request on to its last target:
-- `throttled`, `timeout`, `transport`, `server_error`, `refusal` or
-- `refusal:<category>` (the category being the code or filter category the
-- provider reported). It is what tells a request that reached an expensive
-- fallback target because of a content refusal from one that reached it
-- because of load. Empty when the first dial answered, and for every row older
-- than the column.
--
-- A gateway inserts with `input_format_skip_unknown_fields=1` and leaves the
-- field out of a row that never failed over, so one upgraded before this is
-- applied keeps logging and loses only the reason. The control plane reads the
-- column only when the table has it (an empty string otherwise), so applying
-- this file is not a precondition of upgrading it.

alter table request_logs
    add column if not exists fallback_reason LowCardinality(String) default '';
