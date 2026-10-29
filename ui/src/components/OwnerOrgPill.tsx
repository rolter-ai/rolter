import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { Pill } from "@/components/screen";
import { fetchOrgs } from "@/lib/api";

/**
 * Which organization a route or provider belongs to, for a screen that shows
 * rows of several organizations side by side. Two organizations may each have
 * a route or provider of one name (#1857), and without this their cards read
 * the same (#3460). `orgId` is `""` for a row from the gateway's own config
 * file, which belongs to the whole deployment.
 *
 * The names come from the same organization list the scope switcher reads, so
 * the pill costs no request of its own; an organization the list does not
 * hold is named by the start of its id.
 */
export function OwnerOrgPill({ orgId }: { orgId: string }) {
  const owner = useOwnerLabel(true);
  return (
    <Pill verbatim color="var(--text-secondary)" tint="var(--surface-subtle)">
      {owner(orgId)}
    </Pill>
  );
}

/**
 * The words {@link OwnerOrgPill} shows for an owner, for an accessible name
 * that has to tell two same-named rows apart. `enabled` is false on a screen
 * where no name is shared, so it asks for nothing it will not show.
 */
export function useOwnerLabel(enabled: boolean): (orgId: string) => string {
  const { t } = useTranslation();
  const orgs = useQuery({ queryKey: ["scope", "orgs"], queryFn: fetchOrgs, enabled });
  const list = Array.isArray(orgs.data) ? orgs.data : [];
  return (orgId) =>
    orgId
      ? t("common.ownerOrg", {
          name: list.find((o) => o.id === orgId)?.name ?? orgId.slice(0, 8),
        })
      : t("common.ownerDeployment");
}

/**
 * The names held by more than one organization among `rows`, which are the
 * ones whose cards need an {@link OwnerOrgPill} to tell them apart.
 */
export function sharedNames(rows: { name: string; orgId: string }[]): Set<string> {
  const owners = new Map<string, Set<string>>();
  for (const row of rows) {
    const set = owners.get(row.name) ?? new Set<string>();
    set.add(row.orgId);
    owners.set(row.name, set);
  }
  return new Set([...owners].filter(([, orgs]) => orgs.size > 1).map(([name]) => name));
}
