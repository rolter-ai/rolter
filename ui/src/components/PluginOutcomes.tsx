import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { AlertTriangle } from "lucide-react";
import { useTranslation } from "react-i18next";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { OUTCOME_WINDOW } from "@/components/GuardrailOutcomes";
import {
  AnalyticsUnavailableError,
  fetchPluginOutcomes,
  type PluginOutcomeRow,
  type PluginOutcomes,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { windowBounds } from "@/lib/time-window";

export type PluginOutcomesQuery = UseQueryResult<PluginOutcomes>;

/**
 * Per-plugin call counts over {@link OUTCOME_WINDOW} (#2243), read once per
 * screen and shared by every card on it. The window is the guardrail screens'.
 */
export function usePluginOutcomes(): PluginOutcomesQuery {
  return useQuery({
    queryKey: ["plugin-outcomes", OUTCOME_WINDOW],
    queryFn: () => fetchPluginOutcomes(windowBounds(OUTCOME_WINDOW)),
    // a deployment without ClickHouse says so every time; asking again is noise
    retry: (count, error) => !(error instanceof AnalyticsUnavailableError) && count < 2,
    staleTime: 60_000,
  });
}

/**
 * What the screen says once, above its cards, when the counts cannot be shown:
 * a deployment with no ClickHouse, or a ClickHouse that has not had the file
 * that keeps plugin calls applied. Nothing otherwise.
 */
export function PluginOutcomesNotice({ query }: { query: PluginOutcomesQuery }) {
  const { t } = useTranslation();
  if (query.error instanceof AnalyticsUnavailableError) {
    return <AnalyticsUnavailable error={query.error} i18nKey="pages.pluginOutcomes.noAnalytics" />;
  }
  if (query.data && !query.data.recorded) {
    return (
      <p
        role="status"
        className="max-w-[72ch] rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-4 py-3 text-sm text-muted-foreground"
      >
        {t("pages.pluginOutcomes.notRecorded")}
      </p>
    );
  }
  return null;
}

const num = (value: number | string | undefined): number => Number(value) || 0;

/**
 * True when every call the plugin got in the window failed to get an answer.
 * Only an enabled plugin is flagged: a paused one is not being consulted, so a
 * flag on it would describe traffic it no longer sees.
 */
export function failsEveryCall(row: PluginOutcomeRow | undefined, enabled: boolean): boolean {
  if (!enabled || !row) return false;
  const calls = num(row.calls);
  return calls > 0 && num(row.errors) >= calls;
}

/** `status_503` is "answered with status 503"; the rest are a fixed vocabulary */
function useErrorKind(): (kind: string) => string {
  const { t } = useTranslation();
  return (kind) => {
    const status = /^status_(\d{3})$/.exec(kind);
    if (status) return t("pages.pluginOutcomes.kinds.status", { code: status[1] });
    if (["refused", "timeout", "connect", "body"].includes(kind)) {
      return t(`pages.pluginOutcomes.kinds.${kind}`);
    }
    return t("pages.pluginOutcomes.kinds.unknown");
  };
}

/**
 * The activity block of a plugin card (#2243): what the plugin did over the
 * last seven days, its newest failure, and a flag when it failed every call
 * together with what that does to traffic under its failure policy. A card
 * whose counts cannot be shown says nothing here, since
 * {@link PluginOutcomesNotice} already said why.
 */
export function PluginActivity({
  query,
  orgId,
  slug,
  enabled,
  failureMode,
}: {
  query: PluginOutcomesQuery;
  orgId: string;
  slug: string;
  enabled: boolean;
  failureMode: "fail_open" | "fail_closed";
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const errorKind = useErrorKind();
  if (query.isPending) {
    return (
      <p className="mt-2 text-xs text-muted-foreground">{t("pages.pluginOutcomes.loading")}</p>
    );
  }
  if (query.error instanceof AnalyticsUnavailableError) return null;
  if (query.isError) {
    return <p className="mt-2 text-xs text-muted-foreground">{t("pages.pluginOutcomes.failed")}</p>;
  }
  if (!query.data.recorded) return null;
  const row = query.data.data.find((r) => r.org_id === orgId && r.plugin === slug);
  const calls = num(row?.calls);
  if (!row || calls === 0) {
    return <p className="mt-2 text-xs text-muted-foreground">{t("pages.pluginOutcomes.none")}</p>;
  }
  const errors = num(row.errors);
  const counts: Array<{ key: string; value: number }> = [
    { key: "calls", value: calls },
    { key: "blocked", value: num(row.blocked) },
    { key: "transformed", value: num(row.transformed) },
    ...(num(row.annotated) > 0 ? [{ key: "annotated", value: num(row.annotated) }] : []),
    { key: "errors", value: errors },
  ];
  return (
    <div className="mt-2 text-xs">
      <p className="text-muted-foreground">{t("pages.pluginOutcomes.heading")}</p>
      <ul className="mt-1 flex flex-wrap gap-x-4 gap-y-1">
        {counts.map(({ key, value }) => (
          <li key={key}>
            {t(`pages.pluginOutcomes.counts.${key}`)}{" "}
            <span className="font-medium text-foreground">{fmt.number(value)}</span>
          </li>
        ))}
      </ul>
      {errors > 0 && row.last_error_at && (
        <p className="mt-1 text-muted-foreground">
          {t("pages.pluginOutcomes.lastError", {
            kind: errorKind(row.last_error_kind),
            when: fmt.relative(row.last_error_at),
          })}
        </p>
      )}
      {failsEveryCall(row, enabled) && (
        <p className="mt-2 flex items-start gap-1.5 text-[color:var(--status-danger-text)]">
          <AlertTriangle className="mt-px h-3.5 w-3.5 flex-none" aria-hidden />
          <span>
            {t("pages.pluginOutcomes.failing", {
              errors: fmt.number(errors),
              calls: fmt.number(calls),
            })}{" "}
            {t(`pages.pluginOutcomes.failingEffect.${failureMode}`)}
          </span>
        </p>
      )}
    </div>
  );
}
