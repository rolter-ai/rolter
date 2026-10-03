-- The org that owns the provider a health event describes (#1908).
--
-- Rows named the provider by display name alone, and names are unique per org
-- only. After org A deleted `openai` and org B created its own, B's viewers
-- read A's history for the same name until the 90-day ttl aged it out. Reads
-- now match on (org_id, provider). An empty string means no org: a provider
-- from the gateway's own config file, or a row written before this column
-- existed. Those rows stay visible to unrestricted callers only.

alter table provider_health_events
    add column if not exists org_id String default '';
