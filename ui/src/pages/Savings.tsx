import { useQuery } from "@tanstack/react-query";
import { Scale, TriangleAlert } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { Link, useSearchParams } from "react-router";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { LoadError } from "@/components/LoadError";
import { StatGridSkeleton, TableSkeleton } from "@/components/LoadingState";
import { useOrgScope } from "@/components/OrgScopePicker";
import {
  ListCell,
  ListHeader,
  ListHeaderCell,
  ListRow,
  ListTable,
  PageBody,
  primaryColumn,
} from "@/components/screen";
import { SampledLogNotice } from "@/components/SampledLogNotice";
import { Button } from "@/components/ui/button";
import { TimeWindowPicker } from "@/components/TimeWindowPicker";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { STAT_GRID, StatCard } from "@/components/ui/stat-card";
import { SurfacePanel } from "@/components/ui/surface-panel";
import {
  AnalyticsUnavailableError,
  fetchModels,
  fetchSavings,
  fetchVirtualKeys,
  type MoneyRange,
  type SavingsCandidate,
  type SavingsReport,
} from "@/lib/api";
import { useCan } from "@/lib/can";
import { useFormat } from "@/lib/i18n/format";
import { EXCLUDED_REASONS } from "@/lib/savings-reasons";
import { useScope } from "@/lib/scope";
import {
  readWindowSelection,
  windowAddress,
  windowBounds,
  type TimeWindow,
  useWindowQueryKey,
} from "@/lib/time-window";
import { useScreenReady } from "@/lib/ux-react";

// the advisor reads a week unless it is told otherwise, and a day of traffic is too thin to
// price a swap on, so this screen opens on the same seven days rather than the 24 hours
// the other analytics screens default to
const DEFAULT_WINDOW: TimeWindow = "7d";

/** the address parameters the scope and thresholds travel in, under the control plane's names */
const SCOPE_PARAMS = ["org", "project", "route", "key"] as const;
const THRESHOLD_PARAMS = ["max_prompt_tokens", "max_completion_tokens", "output_margin"] as const;

// the largest margin the control plane honours; a larger one is clamped to it
const MAX_OUTPUT_MARGIN = 4;

const SPEND_GRID = `${primaryColumn(2)} 110px 130px 110px`;
const CALLER_GRID = `${primaryColumn(2)} ${primaryColumn(1.4)} 110px 130px 110px`;
const CANDIDATE_GRID = `${primaryColumn(2)} 1.3fr 1.3fr`;
const EXCLUDED_GRID = `${primaryColumn(3)} 130px`;

/**
 * The savings advisor (#3316): where a window's spend went, and what the part
 * of it that could have run on a cheaper model already in the same route's
 * pool would have cost there.
 *
 * It re-prices and does not judge quality, so the report's own `limits` are the
 * first thing on the screen, and every projected cost or saving is a range.
 * It reads ClickHouse like the other analytics screens, so a deployment
 * without one states that rather than failing.
 */
export default function Savings() {
  const { t } = useTranslation();
  const [params, setParams] = useSearchParams();
  const scope = useScope();
  const orgScope = useOrgScope(scope.orgId);
  const can = useCan();

  const timeWindow = readWindowSelection(params, DEFAULT_WINDOW);
  // the window's days are the chart zone's, so a zone change reads it again (#3519)
  const windowKey = useWindowQueryKey(timeWindow);
  const value = (name: string) => params.get(name) ?? "";
  const filters = {
    org: value("org"),
    project: value("project"),
    route: value("route"),
    key: value("key"),
  };
  const thresholds = {
    max_prompt_tokens: value("max_prompt_tokens"),
    max_completion_tokens: value("max_completion_tokens"),
    output_margin: value("output_margin"),
  };

  const write = (changes: Record<string, string>) =>
    setParams(
      (prev) => {
        // the other parameters are left alone, since the address is not only ours
        const next = new URLSearchParams(prev);
        for (const [name, v] of Object.entries(changes)) {
          if (v) next.set(name, v);
          else next.delete(name);
        }
        return next;
      },
      { replace: true },
    );

  // the pickers' lists. a read the caller may not make just leaves its picker empty
  const models = useQuery({ queryKey: ["models"], queryFn: fetchModels, retry: false });
  const keyProject = filters.project || scope.projectId;
  const keys = useQuery({
    queryKey: ["virtual-keys", keyProject],
    queryFn: () => fetchVirtualKeys(keyProject as string),
    enabled: can("virtual_key", "read") !== false && !!keyProject,
    retry: false,
    staleTime: 60_000,
  });

  const report = useQuery({
    queryKey: [
      "savings",
      ...windowKey,
      filters.org,
      filters.project,
      filters.route,
      filters.key,
      thresholds.max_prompt_tokens,
      thresholds.max_completion_tokens,
      thresholds.output_margin,
    ],
    // the bounds are worked out as the read leaves, so a tab left open reads the window as it is now
    queryFn: () =>
      fetchSavings({
        ...windowBounds(timeWindow),
        org: filters.org || undefined,
        project: filters.project || undefined,
        route: filters.route || undefined,
        key: filters.key || undefined,
        max_prompt_tokens: thresholds.max_prompt_tokens
          ? Number(thresholds.max_prompt_tokens)
          : undefined,
        max_completion_tokens: thresholds.max_completion_tokens
          ? Number(thresholds.max_completion_tokens)
          : undefined,
        output_margin: thresholds.output_margin ? Number(thresholds.output_margin) : undefined,
      }),
    retry: false,
  });
  useScreenReady(!report.isLoading);

  const filtersActive =
    SCOPE_PARAMS.some((name) => params.get(name)) ||
    THRESHOLD_PARAMS.some((name) => params.get(name));
  const clearFilters = () =>
    write(Object.fromEntries([...SCOPE_PARAMS, ...THRESHOLD_PARAMS].map((name) => [name, ""])));

  const options = (names: string[], current: string) => {
    const all = current && !names.includes(current) ? [...names, current] : names;
    return all.map((name) => ({ value: name, label: name }));
  };
  const projects = orgScope.byTeam.flatMap((entry) => entry.projects);
  // the lists the pickers above are drawn from also name the callers in the
  // spend table. a caller's key is named by the report itself, for every
  // project it lists (#3499), and the key list is only the fallback for a
  // control plane that does not; a project or key outside both (one the
  // account cannot read, or one since deleted) stays an id there (#3408)
  const projectNames = new Map(projects.map((project) => [project.id, project.name]));
  const keyNames = new Map((keys.data ?? []).map((k) => [k.id, keyLabel(k)]));
  const unavailable = report.error instanceof AnalyticsUnavailableError ? report.error : null;

  return (
    <PageBody>
      <LimitsNotice limits={report.data?.limits} />
      <SampledLogNotice kind="figures" />

      <div className="flex flex-wrap items-end gap-3">
        <Field label={t("common.timeWindow.label")} className="w-48">
          <TimeWindowPicker
            value={timeWindow}
            onChange={(next) => write(windowAddress(next, DEFAULT_WINDOW))}
          />
        </Field>
        <Field label={t("pages.savings.org")} className="w-48">
          <Combobox
            value={filters.org}
            onChange={(next) => write({ org: next })}
            placeholder={t("pages.savings.allOrgs")}
            options={[
              { value: "", label: t("pages.savings.allOrgs") },
              ...scope.orgs.map((org) => ({ value: org.id, label: org.name })),
            ]}
          />
        </Field>
        <Field label={t("pages.savings.project")} className="w-48">
          <Combobox
            value={filters.project}
            onChange={(next) => write({ project: next })}
            placeholder={t("pages.savings.allProjects")}
            options={[
              { value: "", label: t("pages.savings.allProjects") },
              ...projects.map((project) => ({ value: project.id, label: project.name })),
            ]}
          />
        </Field>
        <Field label={t("pages.savings.route")} className="w-48">
          <Combobox
            value={filters.route}
            onChange={(next) => write({ route: next })}
            placeholder={t("pages.savings.allRoutes")}
            options={[
              { value: "", label: t("pages.savings.allRoutes") },
              ...options(
                // route names are unique per project only, and the report filters on the
                // name, so a picked project narrows the list and a name is offered once
                [
                  ...new Set(
                    (models.data ?? [])
                      .filter(
                        (m) =>
                          !filters.project || !m.project_id || m.project_id === filters.project,
                      )
                      .map((m) => m.model),
                  ),
                ],
                filters.route,
              ),
            ]}
          />
        </Field>
        <Field label={t("pages.savings.key")} className="w-48">
          <Combobox
            value={filters.key}
            onChange={(next) => write({ key: next })}
            placeholder={t("pages.savings.allKeys")}
            options={[
              { value: "", label: t("pages.savings.allKeys") },
              ...(keys.data ?? []).map((k) => ({
                value: k.id,
                label: keyLabel(k),
              })),
              ...(filters.key && !(keys.data ?? []).some((k) => k.id === filters.key)
                ? [{ value: filters.key, label: filters.key }]
                : []),
            ]}
          />
        </Field>
      </div>

      <ThresholdsForm
        // the form is seeded from the address, so a link or a clear re-seeds it
        key={THRESHOLD_PARAMS.map((name) => thresholds[name]).join("|")}
        values={thresholds}
        onApply={write}
      />

      {unavailable ? (
        <AnalyticsUnavailable error={unavailable} i18nKey="pages.savings.noAnalytics" />
      ) : report.isLoading ? (
        <div className="space-y-4" aria-busy="true">
          <StatGridSkeleton cards={4} />
          <TableSkeleton rows={4} />
        </div>
      ) : report.isError ? (
        <LoadError
          error={report.error}
          resource={t("errors.resources.savings")}
          onRetry={() => void report.refetch()}
          target="savings"
        />
      ) : report.data && report.data.spend.requests === 0 ? (
        <EmptyState
          thread
          uxTarget="savings"
          icon={<Scale />}
          title={t(filtersActive ? "pages.savings.noMatchTitle" : "pages.savings.emptyTitle")}
          description={t(filtersActive ? "pages.savings.noMatchBody" : "pages.savings.emptyBody")}
          actions={
            filtersActive ? (
              <Button variant="outline" onClick={clearFilters}>
                {t("common.clearFilters")}
              </Button>
            ) : timeWindow !== "30d" ? (
              <Button variant="outline" onClick={() => write(windowAddress("30d", DEFAULT_WINDOW))}>
                {t("pages.savings.widen")}
              </Button>
            ) : undefined
          }
        />
      ) : report.data ? (
        <Report report={report.data} projectNames={projectNames} keyNames={keyNames} />
      ) : null}
    </PageBody>
  );
}

/**
 * What the advisor is and is not, ahead of every figure (#3316). The first
 * sentence is the dashboard's; the list under it is the control plane's own
 * `limits`, which arrive with each answer and are quoted as they are sent.
 */
function LimitsNotice({ limits }: { limits?: string[] }) {
  const { t } = useTranslation();
  return (
    <section
      aria-labelledby="savings-limits-title"
      data-testid="savings-limits"
      className="rounded-lg border border-[color:var(--status-warning)] bg-[color:var(--surface-subtle)] px-4 py-3.5"
    >
      <div className="flex items-start gap-2.5">
        <TriangleAlert
          aria-hidden
          className="mt-0.5 h-4 w-4 flex-none text-[color:var(--status-warning)]"
        />
        <div className="min-w-0 space-y-2">
          <h2 id="savings-limits-title" className="text-sm font-medium text-foreground">
            {t("pages.savings.limitsTitle")}
          </h2>
          <p className="text-sm text-muted-foreground">{t("pages.savings.limitsLead")}</p>
          {limits && limits.length > 0 && (
            <ul className="list-disc space-y-1 pl-5 text-xs text-muted-foreground">
              {limits.map((limit) => (
                <li key={limit}>{limit}</li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </section>
  );
}

/**
 * The three thresholds that decide what counts as substitutable. They apply
 * together, on purpose: a request per keystroke would re-run the report three
 * times for one change of mind.
 */
function ThresholdsForm({
  values,
  onApply,
}: {
  values: Record<(typeof THRESHOLD_PARAMS)[number], string>;
  onApply: (changes: Record<string, string>) => void;
}) {
  const { t } = useTranslation();
  const [draft, setDraft] = React.useState(values);
  const [attempted, setAttempted] = React.useState(false);
  const whole = (v: string) => v.trim() === "" || (/^\d+$/.test(v.trim()) && Number(v) >= 1);
  const margin = (v: string) =>
    v.trim() === "" ||
    (Number.isFinite(Number(v)) && Number(v) >= 0 && Number(v) <= MAX_OUTPUT_MARGIN);
  const errors = {
    max_prompt_tokens: whole(draft.max_prompt_tokens) ? "" : t("pages.savings.errors.whole"),
    max_completion_tokens: whole(draft.max_completion_tokens)
      ? ""
      : t("pages.savings.errors.whole"),
    output_margin: margin(draft.output_margin)
      ? ""
      : t("pages.savings.errors.margin", { max: MAX_OUTPUT_MARGIN }),
  };
  const set = (name: keyof typeof draft) => (e: React.ChangeEvent<HTMLInputElement>) =>
    setDraft((d) => ({ ...d, [name]: e.target.value }));
  return (
    <form
      aria-label={t("pages.savings.thresholdsTitle")}
      className="flex flex-wrap items-end gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        setAttempted(true);
        if (Object.values(errors).some(Boolean)) return;
        onApply({
          max_prompt_tokens: draft.max_prompt_tokens.trim(),
          max_completion_tokens: draft.max_completion_tokens.trim(),
          output_margin: draft.output_margin.trim(),
        });
      }}
    >
      <Field
        label={t("pages.savings.maxPrompt")}
        hint={t("pages.savings.maxPromptHint")}
        error={attempted ? errors.max_prompt_tokens : ""}
        className="w-56"
      >
        <Input
          inputMode="numeric"
          className="font-mono"
          placeholder="4000"
          value={draft.max_prompt_tokens}
          onChange={set("max_prompt_tokens")}
        />
      </Field>
      <Field
        label={t("pages.savings.maxCompletion")}
        hint={t("pages.savings.maxCompletionHint")}
        error={attempted ? errors.max_completion_tokens : ""}
        className="w-56"
      >
        <Input
          inputMode="numeric"
          className="font-mono"
          placeholder="1000"
          value={draft.max_completion_tokens}
          onChange={set("max_completion_tokens")}
        />
      </Field>
      <Field
        label={t("pages.savings.margin")}
        hint={t("pages.savings.marginHint")}
        error={attempted ? errors.output_margin : ""}
        className="w-56"
      >
        <Input
          inputMode="decimal"
          className="font-mono"
          placeholder="0.25"
          value={draft.output_margin}
          onChange={set("output_margin")}
        />
      </Field>
      <Button type="submit" variant="outline">
        {t("pages.savings.recalculate")}
      </Button>
    </form>
  );
}

/** a key as the picker and the spend table name it: its name with the prefix, or the prefix alone */
function keyLabel(k: { name?: string | null; key_prefix: string }): string {
  return k.name ? `${k.name} (${k.key_prefix}…)` : `${k.key_prefix}…`;
}

/**
 * A caller's project or key: its name when the screen's lists hold it, else
 * the id as it came, in the id's own mono type. The id stays in the title of a
 * named cell, so the row can still be matched to the log.
 */
function CallerCell({
  id,
  names,
  name: given,
}: {
  id: string;
  names: ReadonlyMap<string, string>;
  /** the name the report gave, which wins over the screen's own lists */
  name?: string;
}) {
  const name = given || names.get(id);
  return name ? (
    <ListCell className="truncate text-sm" title={id}>
      {name}
    </ListCell>
  ) : (
    <ListCell className="truncate font-mono text-xs">{id || "—"}</ListCell>
  );
}

function Report({
  report,
  projectNames,
  keyNames,
}: {
  report: SavingsReport;
  projectNames: ReadonlyMap<string, string>;
  keyNames: ReadonlyMap<string, string>;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const money = (v: number) => fmt.currency(v, report.currency);
  const count = (v: number) => fmt.number(Math.round(v));
  const { spend } = report;
  return (
    <div className="space-y-6">
      <section aria-labelledby="savings-spend-title" className="space-y-3">
        <h2 id="savings-spend-title" className="text-base font-medium">
          {t("pages.savings.spendTitle")}
        </h2>
        <div className={STAT_GRID}>
          <StatCard label={t("pages.savings.requests")} value={count(spend.requests)} />
          <StatCard
            label={t("pages.savings.spend")}
            value={money(spend.cost_usd)}
            delta={
              spend.unpriced_requests > 0
                ? t("pages.savings.unpriced", { count: spend.unpriced_requests })
                : undefined
            }
          />
          <StatCard
            label={t("pages.savings.multiAttempt")}
            value={count(spend.multi_attempt.requests)}
            delta={money(spend.multi_attempt.cost_usd)}
          />
          <StatCard
            label={t("pages.savings.failed")}
            value={count(spend.failed.requests)}
            delta={t("pages.savings.spentAnyway", { amount: money(spend.failed.cost_usd) })}
          />
          <StatCard label={t("pages.savings.cacheHits")} value={count(spend.cache.hits)} />
          <StatCard
            label={t("pages.savings.cacheMisses")}
            value={count(spend.cache.misses)}
            delta={money(spend.cache.miss_cost_usd)}
          />
        </div>
        <p className="text-xs text-muted-foreground">{t("pages.savings.unpricedNote")}</p>
        <SpendTable
          label={t("pages.savings.byRoute")}
          nameHeader={t("pages.savings.route")}
          rows={spend.by_route.map((row) => ({ ...row, name: row.route }))}
          money={money}
          count={count}
        />
        <SpendTable
          label={t("pages.savings.byModel")}
          nameHeader={t("pages.savings.model")}
          rows={spend.by_model.map((row) => ({ ...row, name: row.model }))}
          money={money}
          count={count}
        />
        {spend.by_caller.length > 0 && (
          <ListTable label={t("pages.savings.byCaller")}>
            <ListHeader grid={CALLER_GRID}>
              <ListHeaderCell>{t("pages.savings.project")}</ListHeaderCell>
              <ListHeaderCell>{t("pages.savings.key")}</ListHeaderCell>
              <ListHeaderCell className="text-right">{t("pages.savings.requests")}</ListHeaderCell>
              <ListHeaderCell className="text-right">{t("pages.savings.spend")}</ListHeaderCell>
              <ListHeaderCell className="text-right">
                {t("pages.savings.unpricedColumn")}
              </ListHeaderCell>
            </ListHeader>
            {spend.by_caller.map((row) => (
              <ListRow key={`${row.project_id}/${row.virtual_key_id}`} grid={CALLER_GRID}>
                <CallerCell id={row.project_id} names={projectNames} />
                <CallerCell
                  id={row.virtual_key_id}
                  names={keyNames}
                  name={row.key_name ?? undefined}
                />
                <ListCell className="text-right font-mono text-xs">{count(row.requests)}</ListCell>
                <ListCell className="text-right font-mono text-xs">{money(row.cost_usd)}</ListCell>
                <ListCell className="text-right font-mono text-xs">
                  {count(row.unpriced_requests)}
                </ListCell>
              </ListRow>
            ))}
          </ListTable>
        )}
      </section>

      <section aria-labelledby="savings-candidates-title" className="space-y-3">
        <h2 id="savings-candidates-title" className="text-base font-medium">
          {t("pages.savings.candidatesTitle")}
        </h2>
        <p className="text-sm text-muted-foreground">{t("pages.savings.candidatesBody")}</p>
        <p data-testid="savings-substitutable" className="text-sm">
          {t("pages.savings.substitutable", {
            count: Math.round(report.substitutable.requests),
            requests: count(report.substitutable.requests),
            cost: money(report.substitutable.current_cost_usd),
            range: rangeText(report.substitutable.projected_savings_usd, money, t),
          })}
        </p>
        {report.candidates.length === 0 ? (
          <EmptyState
            thread
            uxTarget="savings-candidates"
            icon={<Scale />}
            title={t("pages.savings.noCandidatesTitle")}
            description={t("pages.savings.noCandidatesBody")}
          />
        ) : (
          report.candidates.map((candidate) => (
            <CandidateTable
              key={`${candidate.route}/${candidate.current_model}`}
              candidate={candidate}
              money={money}
              count={count}
            />
          ))
        )}
      </section>

      <ExcludedSection report={report} count={count} />
    </div>
  );
}

/** a range as one phrase, so it is never read as a single figure */
function rangeText(
  range: MoneyRange,
  money: (v: number) => string,
  t: (key: string, options?: Record<string, unknown>) => string,
) {
  return t("pages.savings.range", { low: money(range.low), high: money(range.high) });
}

function SpendTable({
  label,
  nameHeader,
  rows,
  money,
  count,
}: {
  label: string;
  nameHeader: string;
  rows: { name: string; requests: number; cost_usd: number; unpriced_requests: number }[];
  money: (v: number) => string;
  count: (v: number) => string;
}) {
  const { t } = useTranslation();
  if (rows.length === 0) return null;
  return (
    <ListTable label={label}>
      <ListHeader grid={SPEND_GRID}>
        <ListHeaderCell>{nameHeader}</ListHeaderCell>
        <ListHeaderCell className="text-right">{t("pages.savings.requests")}</ListHeaderCell>
        <ListHeaderCell className="text-right">{t("pages.savings.spend")}</ListHeaderCell>
        <ListHeaderCell className="text-right">{t("pages.savings.unpricedColumn")}</ListHeaderCell>
      </ListHeader>
      {rows.map((row) => (
        <ListRow key={row.name} grid={SPEND_GRID}>
          <ListCell className="truncate font-mono text-xs">{row.name}</ListCell>
          <ListCell className="text-right font-mono text-xs">{count(row.requests)}</ListCell>
          <ListCell className="text-right font-mono text-xs">{money(row.cost_usd)}</ListCell>
          <ListCell className="text-right font-mono text-xs">
            {count(row.unpriced_requests)}
          </ListCell>
        </ListRow>
      ))}
    </ListTable>
  );
}

function CandidateTable({
  candidate,
  money,
  count,
}: {
  candidate: SavingsCandidate;
  money: (v: number) => string;
  count: (v: number) => string;
}) {
  const { t } = useTranslation();
  return (
    <SurfacePanel className="space-y-3 p-4">
      <div>
        <h3 className="text-sm font-medium">
          <span className="font-mono">{candidate.route}</span>
          <span className="text-muted-foreground"> · </span>
          <span className="font-mono">{candidate.current_model}</span>
        </h3>
        <p className="text-xs text-muted-foreground">
          {t("pages.savings.currentCost", {
            requests: count(candidate.requests),
            cost: money(candidate.current_cost_usd),
          })}
        </p>
      </div>
      <ListTable label={`${candidate.route} · ${candidate.current_model}`}>
        <ListHeader grid={CANDIDATE_GRID}>
          <ListHeaderCell>{t("pages.savings.model")}</ListHeaderCell>
          <ListHeaderCell className="text-right">{t("pages.savings.projected")}</ListHeaderCell>
          <ListHeaderCell className="text-right">{t("pages.savings.saving")}</ListHeaderCell>
        </ListHeader>
        {candidate.candidates.map((option) => (
          <ListRow key={option.model} grid={CANDIDATE_GRID}>
            <ListCell className="truncate font-mono text-xs">{option.model}</ListCell>
            <ListCell className="text-right font-mono text-xs">
              {t("pages.savings.range", {
                low: money(option.projected_cost_usd.low),
                high: money(option.projected_cost_usd.high),
              })}
            </ListCell>
            <ListCell className="text-right font-mono text-xs">
              {t("pages.savings.range", {
                low: money(option.savings_usd.low),
                high: money(option.savings_usd.high),
              })}
            </ListCell>
          </ListRow>
        ))}
      </ListTable>
    </SurfacePanel>
  );
}

function ExcludedSection({
  report,
  count,
}: {
  report: SavingsReport;
  count: (v: number) => string;
}) {
  const { t } = useTranslation();
  const known = new Set<string>(EXCLUDED_REASONS);
  const reasons = [
    ...EXCLUDED_REASONS.filter((reason) => report.excluded.requests[reason] !== undefined),
    // a reason a newer control plane added is shown under its own name
    ...Object.keys(report.excluded.requests).filter((reason) => !known.has(reason)),
  ];
  const unknown = report.excluded.unknown_candidate_prices;
  return (
    <section aria-labelledby="savings-excluded-title" className="space-y-3">
      <h2 id="savings-excluded-title" className="text-base font-medium">
        {t("pages.savings.excludedTitle")}
      </h2>
      <p className="text-sm text-muted-foreground">{t("pages.savings.excludedBody")}</p>
      {reasons.length > 0 && (
        <ListTable label={t("pages.savings.excludedTitle")}>
          <ListHeader grid={EXCLUDED_GRID}>
            <ListHeaderCell>{t("pages.savings.reason")}</ListHeaderCell>
            <ListHeaderCell className="text-right">{t("pages.savings.requests")}</ListHeaderCell>
          </ListHeader>
          {reasons.map((reason) => (
            <ListRow key={reason} grid={EXCLUDED_GRID}>
              <ListCell>
                {known.has(reason) ? t(`pages.savings.reasons.${reason}`) : reason}
              </ListCell>
              <ListCell className="text-right font-mono text-xs">
                {count(report.excluded.requests[reason])}
              </ListCell>
            </ListRow>
          ))}
        </ListTable>
      )}
      {unknown.length > 0 && (
        <div
          data-testid="savings-unknown-prices"
          className="rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-4 py-3"
        >
          <p className="text-sm font-medium">{t("pages.savings.unknownPricesTitle")}</p>
          <p className="text-sm text-muted-foreground">{t("pages.savings.unknownPricesBody")}</p>
          <ul className="mt-2 flex flex-wrap gap-2">
            {unknown.map((model) => (
              <li key={model}>
                <Link
                  // the editor opens on this model (#3422)
                  to={`/pricing-overrides?${new URLSearchParams({ model })}`}
                  className="font-mono text-xs underline underline-offset-2"
                >
                  {model}
                </Link>
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}
