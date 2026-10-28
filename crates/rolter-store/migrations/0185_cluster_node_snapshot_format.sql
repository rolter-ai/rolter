-- the snapshot formats a node states it reads on its poll (`formats`), and the
-- format it was last served (ADR-0038, #3351). A control plane may serve an
-- older gateway an older format, and a down-step may leave out what that format
-- cannot carry, so a node on the current config_version is not necessarily
-- serving all of it. A poll with no `formats` is a gateway from before
-- negotiation, which reads format 1, hence the defaults. `served_format` is null
-- while the node's last poll was refused for its format.
--
-- No bump_config_version() trigger: the gateway does not read this table.
alter table cluster_nodes
    add column if not exists formats_min integer not null default 1,
    add column if not exists formats_max integer not null default 1,
    add column if not exists served_format integer,
    add constraint cluster_nodes_formats_range
        check (formats_min >= 1 and formats_max >= formats_min),
    add constraint cluster_nodes_served_format_positive
        check (served_format is null or served_format >= 1);
