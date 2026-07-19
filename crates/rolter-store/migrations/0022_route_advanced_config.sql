-- Per-route catalog policy. Keeping this JSONB lets the control plane evolve
-- model settings without mixing them with provider credentials/defaults.
alter table routes
    add column if not exists advanced jsonb not null default '{}'::jsonb;
