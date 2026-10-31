import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import {
  fetchBusinessUnits,
  fetchCustomers,
  fetchVirtualKeys,
  type AlertChannelRow,
  type AlertScopeType,
} from "@/lib/api";
import { useScope } from "@/lib/scope";

/** the scope a rule is created on: a scope type, or the whole deployment */
export type AlertScopeChoice = AlertScopeType | "deployment";

export interface AlertScopeOption {
  value: string;
  label: string;
}

/**
 * The entities a rule can watch for one scope type (#1829), from the places the
 * dashboard already reads: the scope switcher's orgs, teams and projects, and
 * the current project's keys and the current org's business units and customers.
 *
 * `retry: false` on the org lookups for the reason Limits gives: a caller
 * without read access gets a 403 that asking again will not change, and the
 * picker then offers nothing rather than spinning.
 */
export function useAlertScopeOptions(type: AlertScopeChoice): {
  options: AlertScopeOption[];
  pending: boolean;
} {
  const scope = useScope();
  const keys = useQuery({
    queryKey: ["virtual-keys", scope.projectId],
    queryFn: () => fetchVirtualKeys(scope.projectId as string),
    enabled: type === "virtual_key" && !!scope.projectId,
    retry: false,
  });
  const units = useQuery({
    queryKey: ["business-units", scope.orgId],
    queryFn: () => fetchBusinessUnits(scope.orgId as string),
    enabled: type === "business_unit" && !!scope.orgId,
    retry: false,
  });
  const customers = useQuery({
    queryKey: ["customers", scope.orgId],
    queryFn: () => fetchCustomers(scope.orgId as string),
    enabled: type === "customer" && !!scope.orgId,
    retry: false,
  });
  const named = (rows: { id: string; name: string }[]) =>
    rows.map((row) => ({ value: row.id, label: row.name }));
  switch (type) {
    case "org":
      return { options: named(scope.orgs), pending: scope.isLoading };
    case "team":
      return { options: named(scope.teams), pending: scope.isLoading };
    case "project":
      return { options: named(scope.projects), pending: scope.isLoading };
    case "virtual_key":
      return {
        options: (keys.data ?? []).map((k) => ({ value: k.id, label: k.name || k.key_prefix })),
        pending: keys.isLoading,
      };
    case "business_unit":
      return { options: named(units.data ?? []), pending: units.isLoading };
    case "customer":
      return { options: named(customers.data ?? []), pending: customers.isLoading };
    default:
      return { options: [], pending: false };
  }
}

/**
 * The org a rule on this scope would belong to, for narrowing the channels it
 * may notify: the picked org itself, otherwise the org the switcher is on,
 * since every other entity offered sits in it. `null` is the deployment.
 */
export function ruleOrgFor(
  type: AlertScopeChoice,
  entityId: string,
  currentOrgId: string | undefined,
): string | null {
  if (type === "deployment") return null;
  return type === "org" ? entityId || null : (currentOrgId ?? null);
}

/**
 * Whether a rule owned by `ruleOrg` may notify `channel`: a channel of its own
 * org or a deployment one, and a deployment-wide rule only a deployment one.
 * The control plane enforces the same rule; this keeps the picker from offering
 * what it would refuse.
 */
export function channelFits(channel: AlertChannelRow, ruleOrg: string | null): boolean {
  return !channel.org_id || channel.org_id === ruleOrg;
}

/** A rule's scope by name where the switcher knows it, else by the start of its id. */
export function useAlertScopeName(): (type: string, id: string) => string {
  const scope = useScope();
  const { t } = useTranslation();
  return (type, id) => {
    const rows =
      type === "org"
        ? scope.orgs
        : type === "team"
          ? scope.teams
          : type === "project"
            ? scope.projects
            : [];
    const name = rows.find((row) => row.id === id)?.name ?? id.slice(0, 8);
    return t("pages.alerting.scope.named", {
      type: t(`pages.alerting.scope.types.${type}`),
      name,
    });
  };
}
