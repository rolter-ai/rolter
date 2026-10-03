import type { GatewayConfigDto, ProviderGroupRow, ProviderRow } from "@/lib/api";

/** one route or group that points at a provider */
export interface UsageEntry {
  name: string;
  /**
   * the provider is every target or member it has, so taking the provider out
   * leaves it with none
   */
  only: boolean;
}

export interface ProviderUsage {
  routes: UsageEntry[];
  groups: UsageEntry[];
}

const byName = (a: UsageEntry, b: UsageEntry) => a.name.localeCompare(b.name);

/**
 * The routes and provider groups that point at `provider` (#2143).
 *
 * Routes are read from the effective config, which names each target's provider
 * rather than its row id. Provider names are unique across the deployment, so
 * the name is a safe key. Groups carry their members' ids.
 *
 * A model name can repeat across projects: it is one line here, since the name
 * is what the reader goes looking for, and it is flagged when any of them would
 * be left with no target.
 */
export function providerUsage(
  provider: Pick<ProviderRow, "id" | "name">,
  config: Pick<GatewayConfigDto, "routes"> | undefined,
  groups: readonly ProviderGroupRow[] | undefined,
): ProviderUsage {
  const routes = new Map<string, boolean>();
  for (const route of config?.routes ?? []) {
    const targets = route.targets ?? [];
    const mine = targets.filter((target) => target.provider === provider.name);
    if (mine.length === 0) continue;
    routes.set(route.model, (routes.get(route.model) ?? false) || mine.length === targets.length);
  }
  const memberOf: UsageEntry[] = [];
  for (const group of groups ?? []) {
    const mine = group.members.filter((member) => member.provider_id === provider.id);
    if (mine.length === 0) continue;
    memberOf.push({ name: group.name, only: mine.length === group.members.length });
  }
  return {
    routes: [...routes].map(([name, only]) => ({ name, only })).sort(byName),
    groups: memberOf.sort(byName),
  };
}
