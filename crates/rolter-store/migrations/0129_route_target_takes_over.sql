-- the failures a route target is a fallback for (#1584). a target with a
-- non-empty set is never dialled first and is tried only after another
-- target failed with one of these classes; the empty default keeps every
-- existing target an ordinary member of its pool. each entry is a class token
-- (`throttled`, `timeout`, `transport`, `server_error`, `refusal` or
-- `refusal:<category>`), validated by the control plane before it is stored.
-- route_targets already carries the bump_config_version() trigger (0003), so
-- a change reaches the snapshot without a new one
alter table route_targets add column if not exists takes_over text[] not null default '{}';
