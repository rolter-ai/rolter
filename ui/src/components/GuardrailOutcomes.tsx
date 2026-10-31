import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import {
  AnalyticsUnavailableError,
  fetchGuardrailOutcomes,
  type GuardrailAction,
  type GuardrailOutcomes,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { windowBounds, type TimeWindow } from "@/lib/time-window";

/**
 * The window the guardrail screens count matches over (#2158). Stated on every
 * card, and the same window the card's links open LLM Logs on, so the number
 * and the rows behind it are the same set.
 */
export const OUTCOME_WINDOW: TimeWindow = "7d";

/** the actions a rule can take on a request, in the order a card lists them */
export const RULE_ACTIONS = [
  "blocked",
  "redacted",
  "annotated",
] as const satisfies GuardrailAction[];

/** the outcomes a guardrail provider can record, including a local scoring cut */
export const PROVIDER_ACTIONS = [
  "provider_blocked",
  "provider_transformed",
  "semantic_truncated",
] as const satisfies GuardrailAction[];

export type OutcomesQuery = UseQueryResult<GuardrailOutcomes>;

/**
 * Per-rule outcome counts over {@link OUTCOME_WINDOW}, read once per screen and
 * shared by every card on it.
 */
export function useGuardrailOutcomes(): OutcomesQuery {
  return useQuery({
    queryKey: ["guardrail-outcomes", OUTCOME_WINDOW],
    queryFn: () => fetchGuardrailOutcomes(windowBounds(OUTCOME_WINDOW)),
    // a deployment without ClickHouse says so every time; asking again is noise
    retry: (count, error) => !(error instanceof AnalyticsUnavailableError) && count < 2,
    staleTime: 60_000,
  });
}

/** the address of the LLM Logs rows one outcome counted */
export function outcomeLogsHref(action: GuardrailAction, rule: string): string {
  const event = rule ? `${action}:${rule}` : action;
  return `/logs?window=${OUTCOME_WINDOW}&guardrail=${encodeURIComponent(event)}`;
}

/**
 * What the screen says once, above its cards, when the counts cannot be shown:
 * a deployment with no ClickHouse, or a ClickHouse that has not had the file
 * that keeps outcomes applied. Nothing otherwise.
 */
export function GuardrailOutcomesNotice({ query }: { query: OutcomesQuery }) {
  const { t } = useTranslation();
  if (query.error instanceof AnalyticsUnavailableError) {
    return (
      <AnalyticsUnavailable error={query.error} i18nKey="pages.guardrailOutcomes.noAnalytics" />
    );
  }
  if (query.data && !query.data.recorded) {
    return (
      <p
        role="status"
        className="max-w-[72ch] rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-4 py-3 text-sm text-muted-foreground"
      >
        {t("pages.guardrailOutcomes.notRecorded")}
      </p>
    );
  }
  return null;
}

/** how many requests the rule took `action` on; 0 for one that never did */
function countOf(data: GuardrailOutcomes, action: GuardrailAction, rule: string): number {
  const row = data.data.find((r) => r.action === action && r.rule === rule);
  return row ? Number(row.requests) || 0 : 0;
}

/**
 * The outcome line of a card: what `rule` did over the last seven days, each
 * non-zero count a link to the LLM Logs rows behind it. `rule` is empty for the
 * guardrail provider. A card whose counts cannot be shown says nothing here,
 * since {@link GuardrailOutcomesNotice} already said why.
 */
export function OutcomeCounts({
  query,
  rule,
  actions,
}: {
  query: OutcomesQuery;
  rule: string;
  actions: readonly GuardrailAction[];
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  if (query.isPending) {
    return <p className="mt-2 text-muted-foreground">{t("pages.guardrailOutcomes.loading")}</p>;
  }
  if (query.error instanceof AnalyticsUnavailableError) return null;
  if (query.isError) {
    return <p className="mt-2 text-muted-foreground">{t("pages.guardrailOutcomes.failed")}</p>;
  }
  if (!query.data.recorded) return null;
  const counts = actions.map((action) => ({ action, count: countOf(query.data, action, rule) }));
  if (counts.every((c) => c.count === 0)) {
    return <p className="mt-2 text-muted-foreground">{t("pages.guardrailOutcomes.none")}</p>;
  }
  return (
    <div className="mt-2">
      <p className="text-muted-foreground">{t("pages.guardrailOutcomes.heading")}</p>
      <ul className="mt-1 flex flex-wrap gap-x-4 gap-y-1">
        {counts.map(({ action, count }) => (
          <li key={action}>
            {t(`pages.guardrailOutcomes.actions.${action}`)}{" "}
            {count > 0 ? (
              <Link
                to={outcomeLogsHref(action, rule)}
                className="font-medium text-foreground underline underline-offset-2"
                aria-label={t("pages.guardrailOutcomes.viewAria", {
                  total: count,
                  action: t(`pages.guardrailOutcomes.actions.${action}`),
                })}
              >
                {fmt.number(count)}
              </Link>
            ) : (
              <span>{fmt.number(0)}</span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

/**
 * What the enforced provider's card says about outcomes recorded without a
 * provider name: rows written before the gateway named the provider (#3038), or
 * by a config-file webhook that sets no `name`. They cannot be tied to one
 * provider, so they are stated apart from the card's own counts rather than
 * added to them, and the links open exactly those rows. Nothing when there are
 * none.
 */
export function UnnamedProviderCounts({ query }: { query: OutcomesQuery }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  if (!query.data?.recorded) return null;
  const counts = PROVIDER_ACTIONS.map((action) => ({
    action,
    count: countOf(query.data, action, ""),
  })).filter((c) => c.count > 0);
  if (counts.length === 0) return null;
  return (
    <div className="mt-2">
      <p className="text-muted-foreground">{t("pages.guardrailOutcomes.unnamed")}</p>
      <ul className="mt-1 flex flex-wrap gap-x-4 gap-y-1">
        {counts.map(({ action, count }) => (
          <li key={action}>
            {t(`pages.guardrailOutcomes.actions.${action}`)}{" "}
            <Link
              to={outcomeLogsHref(action, "")}
              className="font-medium text-foreground underline underline-offset-2"
              aria-label={t("pages.guardrailOutcomes.viewAria", {
                total: count,
                action: t(`pages.guardrailOutcomes.actions.${action}`),
              })}
            >
              {fmt.number(count)}
            </Link>
          </li>
        ))}
      </ul>
    </div>
  );
}
