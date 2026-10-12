import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeftRight, CircleHelp, Plus } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { IconFrame } from "@/components/ui/icon-frame";
import { CardStack } from "@/components/ui/card";
import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton, FormSkeleton } from "@/components/LoadingState";
import { ListSummary, PageBody, RowIconButton } from "@/components/screen";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { EmptyState, EmptyStateLink } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import { Overline } from "@/components/ui/overline";
import {
  fetchRouteComplexity,
  fetchRoutes,
  setRouteComplexity,
  type ComplexityTier,
  type RouteRow,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useFormat, type Formatters } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";

/**
 * What is known about one route's complexity policy.
 *
 * The four cases are deliberately separate. `unconfigured` is an answer from
 * the control plane; `loading` and `failed` are the absence of one, and folding
 * either into "no policy yet" invites an operator to write a policy over one
 * they were simply never shown (#1461).
 */
type PolicyState =
  | { kind: "loading"; route: RouteRow }
  | { kind: "failed"; route: RouteRow; error: unknown; retry: () => void }
  | { kind: "configured"; route: RouteRow; tiers: ComplexityTier[] }
  | { kind: "unconfigured"; route: RouteRow };

/** `states.filter(isKind("configured"))`, narrowed rather than cast */
function isKind<K extends PolicyState["kind"]>(kind: K) {
  return (state: PolicyState): state is Extract<PolicyState, { kind: K }> => state.kind === kind;
}

// bounded input-size tiers per route: requests below each byte ceiling are
// re-routed to the tier's model; the catch-all tier (no ceiling) closes the
// policy. validated server-side against configured route names.
export default function ComplexityRouter() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const scope = useScope();
  const routes = useQuery({
    queryKey: ["routes", scope.projectId],
    queryFn: () => fetchRoutes(scope.projectId as string),
    enabled: !!scope.projectId,
  });

  const policyQueries = useQueries({
    queries: (routes.data ?? []).map((r) => ({
      queryKey: ["route-complexity", r.id],
      queryFn: () => fetchRouteComplexity(r.id),
    })),
  });

  const [editing, setEditing] = React.useState<RouteRow | null>(null);
  // one request per route, so "this route has no policy" and "this route's
  // policy never arrived" are different answers and the screen has to say which
  // one it is holding. `?? []` collapsed them, and a delayed, 500'd or 403'd
  // read rendered as a route the operator had simply never configured (#1461)
  const states = (routes.data ?? []).map((route, i): PolicyState => {
    const query = policyQueries[i];
    if (!query || query.isPending) return { kind: "loading", route };
    if (query.error) {
      return { kind: "failed", route, error: query.error, retry: () => void query.refetch() };
    }
    const tiers = query.data?.tiers ?? [];
    return tiers.length > 0
      ? { kind: "configured", route, tiers }
      : { kind: "unconfigured", route };
  });
  const configured = states.filter(isKind("configured"));
  const unconfigured = states.filter(isKind("unconfigured"));
  const checking = states.filter(isKind("loading"));
  const failed = states.filter(isKind("failed"));
  // only a route whose policy actually came back counts towards the summary: a
  // denominator that includes the unread ones states them as unconfigured
  const resolved = configured.length + unconfigured.length;

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider.
  // the route list is half of what the reader waits on — the policy reads decide
  // what every card says — so readiness follows both. the error signal is the
  // two `LoadError`s' own, one for the route list and one for the policy group
  useScreenReady(!routes.isLoading && checking.length === 0);

  return (
    <PageBody>
      {/* "0 of 0" is what an unread route list and a route list whose every
          policy is still out or failed both used to print; the summary waits
          for a route list, and for one policy answer when there are routes */}
      <ListSummary data={resolved > 0 || states.length === 0 ? routes.data : undefined}>
        {() =>
          t("pages.complexityRouter.summary", { configured: configured.length, count: resolved })
        }
      </ListSummary>
      {checking.length > 0 && (
        <span className="text-sm text-[color:var(--text-subtle)]">
          {t("pages.complexityRouter.checkingPolicies", { count: checking.length })}
        </span>
      )}
      {failed.length > 0 && (
        <span className="text-sm text-[color:var(--status-danger-text)]">
          {t("pages.complexityRouter.unreadPolicies", { count: failed.length })}
        </span>
      )}

      {routes.isLoading && <CardGridSkeleton cards={3} height={196} min={380} />}
      {routes.error && (
        <LoadError
          error={routes.error}
          resource={t("errors.resources.routes")}
          onRetry={() => void routes.refetch()}
          target="complexity-routes"
        />
      )}
      {routes.isSuccess && states.length === 0 && (
        // a complexity policy hangs off a route, so with no routes there is
        // nothing on this screen to create — the CTA points where it is made
        <EmptyState
          uxTarget="complexity-routes"
          icon={<ArrowLeftRight />}
          title={t("pages.complexityRouter.emptyTitle")}
          description={t("pages.complexityRouter.emptyBody")}
          actions={
            <EmptyStateLink to="/routing-rules">
              {t("pages.complexityRouter.emptyAction")}
            </EmptyStateLink>
          }
        />
      )}
      {failed.length > 0 && (
        <div className="flex flex-col gap-2.5">
          <Overline as="h2">{t("pages.complexityRouter.policyLoadFailed")}</Overline>
          {/* one alert for the group rather than one per route: it is the same
              endpoint failing every time, and the routes it covers are named
              underneath. retry re-runs exactly the reads that failed */}
          {/* load-error-allow: one alert for the per-route policy reads beside the route list, which pairs as complexity-routes; no list of its own */}
          <LoadError
            error={failed[0].error}
            resource={t("errors.resources.complexityPolicies")}
            onRetry={() => failed.forEach((f) => f.retry())}
            target="complexity-policies"
          />
          <div className="flex flex-wrap gap-2.5">
            {failed.map(({ route }) => (
              // deliberately not a button: an editor seeded from a read that
              // failed would save a fresh draft over contents nobody has seen
              <span
                key={route.id}
                title={t("pages.complexityRouter.policyUnknownHint")}
                className="flex items-center gap-2 rounded-[8px] border border-dashed border-[color:var(--border-subtle)] px-3 py-2 font-mono text-xs text-[color:var(--text-subtle)]"
              >
                {route.model}
                <CircleHelp aria-hidden className="h-3 w-3" />
              </span>
            ))}
          </div>
        </div>
      )}

      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(380px,100%),1fr))]">
        {configured.map(({ route, tiers }) => (
          <CardStack key={route.id}>
            <div className="flex items-center gap-2.5">
              <IconFrame className="text-[color:var(--red-folk-text)]">
                <ArrowLeftRight className="h-4 w-4" />
              </IconFrame>
              <span className="min-w-0 truncate font-mono text-sm font-semibold">
                {route.model}
              </span>
            </div>
            <div className="flex flex-col gap-1.5">
              {/* renamed off `t`: the screen now translates its own copy, and a
                  tier shadowing the translator is a trap for the next edit */}
              {tiers.map((tier) => (
                <div
                  key={tier.name}
                  className="flex items-center gap-2 rounded-[8px] bg-[color:var(--surface-subtle)] px-2.5 py-1.5 font-mono text-xs"
                >
                  <span className="text-[color:var(--text-secondary)]">{tier.name}</span>
                  <span className="text-[color:var(--text-subtle)]">
                    {tier.max_input_bytes === null || tier.max_input_bytes === undefined
                      ? t("pages.complexityRouter.catchAll")
                      : `≤ ${formatBytes(fmt, tier.max_input_bytes)}`}
                  </span>
                  <span className="ml-auto truncate text-muted-foreground">→ {tier.route}</span>
                </div>
              ))}
            </div>
            {/* no catch-all means no tier takes the largest requests; they keep
                the route they asked for, which the card says rather than leaves
                to be guessed */}
            {tiers[tiers.length - 1].max_input_bytes != null && (
              <p className="text-xs text-muted-foreground">
                {t("pages.complexityRouter.aboveLast", {
                  size: formatBytes(fmt, tiers[tiers.length - 1].max_input_bytes as number),
                  model: route.model,
                })}
              </p>
            )}
            <div className="flex items-center justify-end border-t border-[color:var(--border-subtle)] pt-3">
              {/* a complexity policy is stored on the route, so editing one
                  is the route's own update capability (#1258) */}
              <GatedButton
                gate="route:update"
                control="complexity-policy-edit"
                size="sm"
                variant="outline"
                aria-label={t("pages.complexityRouter.editPolicyAria", {
                  model: route.model,
                })}
                onClick={() => setEditing(route)}
              >
                {t("pages.complexityRouter.editPolicy")}
              </GatedButton>
            </div>
          </CardStack>
        ))}
      </div>

      {!routes.isLoading && checking.length > 0 && (
        <div className="flex flex-col gap-2.5">
          <Overline as="h2">{t("pages.complexityRouter.checkingPolicy")}</Overline>
          <CardGridSkeleton
            cards={checking.length}
            height={196}
            min={380}
            testId="complexity-policies-loading"
          />
        </div>
      )}

      {unconfigured.length > 0 && (
        <>
          <Overline as="h2" className="mt-2">
            {t("pages.complexityRouter.noPolicyYet")}
          </Overline>
          <div className="flex flex-wrap gap-2.5">
            {/* a policy lives on its route, so this entry point is the same
                `route:update` the edit button on a configured card is */}
            {unconfigured.map(({ route }) => (
              <RowIconButton
                key={route.id}
                gate="route:update"
                control="complexity-policy-new"
                aria-label={t("pages.complexityRouter.addPolicyAria", {
                  model: route.model,
                })}
                onClick={() => setEditing(route)}
                className="gap-2 rounded-[8px] px-3 py-2 font-mono text-xs text-[color:var(--text-secondary)] hover:border-[color:var(--border-default)] disabled:cursor-not-allowed disabled:opacity-50"
              >
                {route.model}
                <Plus className="h-3 w-3" />
              </RowIconButton>
            ))}
          </div>
        </>
      )}

      {editing && (
        <PolicyDialog
          route={editing}
          allRoutes={(routes.data ?? []).map((r) => r.model)}
          onClose={() => setEditing(null)}
        />
      )}
    </PageBody>
  );
}

// IEC symbols stay as they are — the number in front of them is what has to
// follow the dashboard locale
function formatBytes(fmt: Formatters, bytes: number) {
  if (bytes >= 1_048_576)
    return `${fmt.number(bytes / 1_048_576, { maximumFractionDigits: 1 })} MiB`;
  if (bytes >= 1024) return `${fmt.number(Math.round(bytes / 1024))} KiB`;
  return `${fmt.number(bytes)} B`;
}

/** one tier's complaints, each a catalog key under `pages.complexityRouter.validation` */
interface TierErrors {
  name?: string;
  ceiling?: string;
}

/**
 * The server's shape rules, read before the round trip so the row at fault is
 * named instead of a toast naming the tier. Ceilings must climb strictly, a
 * tier with no ceiling closes the policy so only the last may omit one, and
 * names are unique. The server stays the authority.
 */
function checkTiers(tiers: ComplexityTier[]): TierErrors[] {
  const seen = new Set<string>();
  let previous: number | null = null;
  return tiers.map((tier, i) => {
    const errors: TierErrors = {};
    const name = tier.name.trim();
    if (name === "") errors.name = "nameRequired";
    else if (seen.has(name)) errors.name = "nameDuplicate";
    seen.add(name);
    const ceiling = tier.max_input_bytes;
    if (ceiling == null) {
      if (i + 1 !== tiers.length) errors.ceiling = "catchAllNotLast";
    } else if (!Number.isInteger(ceiling) || ceiling <= 0) {
      errors.ceiling = "ceilingPositive";
    } else if (previous != null && ceiling <= previous) {
      errors.ceiling = "ceilingAscending";
    } else {
      previous = ceiling;
    }
    return errors;
  });
}

// name, ceiling, route, remove: one grid for the header and every row
const TIER_GRID =
  "grid grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)_minmax(0,1.6fr)_auto] items-start gap-2";

function PolicyDialog({
  route,
  allRoutes,
  onClose,
}: {
  route: RouteRow;
  allRoutes: string[];
  onClose: () => void;
}) {
  // `t` is the tier in the rows below, so the catalog reader takes another name
  const { t: translate } = useTranslation();
  const fmt = useFormat();
  const base = React.useId();
  const toast = useToast();
  const queryClient = useQueryClient();
  const existing = useQuery({
    queryKey: ["route-complexity", route.id],
    queryFn: () => fetchRouteComplexity(route.id),
    // the editor is about to overwrite this policy, so it reads it again on
    // open rather than seeding a draft from whatever the list cached at page
    // load — and that read is one the sheet has to be able to fail (#1461)
    refetchOnMount: "always",
  });

  const [tiers, setTiers] = React.useState<ComplexityTier[] | null>(null);
  React.useEffect(() => {
    if (existing.data && tiers === null) {
      setTiers(
        existing.data.tiers.length > 0
          ? existing.data.tiers
          : [
              { name: "simple", max_input_bytes: 4096, route: route.model },
              { name: "complex", max_input_bytes: null, route: route.model },
            ],
      );
    }
  }, [existing.data, tiers, route.model]);

  const save = useMutation({
    mutationFn: (next: ComplexityTier[]) => setRouteComplexity(route.id, { tiers: next }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["route-complexity", route.id] });
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({
        tone: "success",
        title: translate("toast.saved"),
        detail: translate("toast.savedDetail", { what: route.model }),
      });
      onClose();
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: translate("toast.saveFailed", { what: route.model }),
        detail: errorText(error, translate),
      });
    },
  });
  const set = (i: number, patch: Partial<ComplexityTier>) =>
    setTiers((ts) => ts?.map((t, j) => (j === i ? { ...t, ...patch } : t)) ?? null);

  const rowErrors = checkTiers(tiers ?? []);
  const invalidRows = rowErrors.filter((e) => e.name || e.ceiling).length;
  const last = tiers?.[tiers.length - 1];
  const dirty = JSON.stringify(tiers) !== JSON.stringify(existing.data?.tiers ?? null);

  return (
    <EditorSheet
      name="complexity-policy-edit"
      open
      onOpenChange={(open) => !open && onClose()}
      title={translate("pages.complexityRouter.policyTitle")}
      subtitle={translate("pages.complexityRouter.policySubtitle", {
        model: route.model,
      })}
      dirty={dirty}
      errorMessage={
        invalidRows > 0
          ? translate("common.fieldsNeedAttention", { count: invalidRows })
          : save.isError
            ? errorText(save.error, translate)
            : undefined
      }
      saveLabel={translate("common.save")}
      canSave={
        !!tiers && tiers.length > 0 && invalidRows === 0 && !existing.isPending && !existing.error
      }
      saving={save.isPending}
      onSave={() => tiers && save.mutate(tiers)}
    >
      {/* the editor seeds itself from the same read the list makes, so it holds
          nothing in the same two ways: still waiting, and failed. it rendered
          the default two-tier draft for both, offering a save that would have
          replaced a policy nobody had seen (#1461) */}
      {existing.isPending && <FormSkeleton fields={3} />}
      {existing.error && (
        // load-error-allow: the editor sheet's seed read; it is one policy, not a list
        <LoadError
          error={existing.error}
          resource={translate("errors.resources.complexityPolicy")}
          onRetry={() => void existing.refetch()}
          target="complexity-policy"
        />
      )}
      {!existing.isPending && !existing.error && (
        <div className="space-y-2.5">
          {/* column headers: a placeholder is gone once a value is typed, and
              a ceiling that reads 4096 must say it is bytes */}
          <div className={`${TIER_GRID} text-xs font-medium text-[color:var(--text-secondary)]`}>
            <span>{translate("pages.complexityRouter.columns.name")}</span>
            <span>{translate("pages.complexityRouter.columns.ceiling")}</span>
            <span>{translate("pages.complexityRouter.columns.route")}</span>
            <span className="w-[30px]" />
          </div>
          {(tiers ?? []).map((t, i) => {
            const errors = rowErrors[i] ?? {};
            const nameErrorId = `${base}-${i}-name-error`;
            const ceilingErrorId = `${base}-${i}-ceiling-error`;
            return (
              <div key={i} className="space-y-1">
                <div className={TIER_GRID}>
                  <Input
                    className="font-mono text-xs"
                    value={t.name}
                    aria-label={translate("pages.complexityRouter.tierNameAria", { n: i + 1 })}
                    aria-invalid={errors.name ? true : undefined}
                    aria-describedby={describedBy(!!errors.name && nameErrorId)}
                    onChange={(e) => set(i, { name: e.target.value })}
                  />
                  <div className="min-w-0">
                    <Input
                      className="font-mono text-xs"
                      type="number"
                      min={1}
                      value={t.max_input_bytes ?? ""}
                      placeholder={translate("pages.complexityRouter.catchAll")}
                      aria-label={translate("pages.complexityRouter.tierCeilingAria", {
                        n: i + 1,
                      })}
                      aria-invalid={errors.ceiling ? true : undefined}
                      aria-describedby={describedBy(!!errors.ceiling && ceilingErrorId)}
                      onChange={(e) =>
                        set(i, {
                          max_input_bytes: e.target.value === "" ? null : Number(e.target.value),
                        })
                      }
                    />
                    <span className="mt-0.5 block text-xs text-[color:var(--text-subtle)]">
                      {t.max_input_bytes == null
                        ? translate("pages.complexityRouter.noCeiling")
                        : `= ${formatBytes(fmt, t.max_input_bytes)}`}
                    </span>
                  </div>
                  <Combobox
                    size="sm"
                    className="min-w-0 font-mono"
                    aria-label={translate("pages.complexityRouter.tierRouteAria", { n: i + 1 })}
                    value={t.route}
                    onChange={(route) => set(i, { route })}
                    options={allRoutes.map((m) => ({ value: m, label: m }))}
                  />
                  <DeleteIconButton
                    label={translate("pages.complexityRouter.removeTierAria", {
                      name: t.name || i + 1,
                    })}
                    onClick={() => setTiers((ts) => ts?.filter((_, j) => j !== i) ?? null)}
                  />
                </div>
                <FieldError
                  id={nameErrorId}
                  error={
                    errors.name && translate(`pages.complexityRouter.validation.${errors.name}`)
                  }
                />
                <FieldError
                  id={ceilingErrorId}
                  error={
                    errors.ceiling &&
                    translate(`pages.complexityRouter.validation.${errors.ceiling}`)
                  }
                />
              </div>
            );
          })}
          {/* where the requests above the last ceiling go: with no catch-all
              they keep the route they asked for */}
          {last && last.max_input_bytes != null && (
            <p className="text-xs text-[color:var(--text-subtle)]">
              {translate("pages.complexityRouter.aboveLast", {
                size: formatBytes(fmt, last.max_input_bytes),
                model: route.model,
              })}
            </p>
          )}
          <Button
            size="sm"
            variant="outline"
            onClick={() =>
              setTiers((ts) => [
                ...(ts ?? []),
                {
                  name: `tier-${(ts?.length ?? 0) + 1}`,
                  max_input_bytes: null,
                  route: route.model,
                },
              ])
            }
          >
            <Plus className="h-3.5 w-3.5" />
            {translate("pages.complexityRouter.addTier")}
          </Button>
        </div>
      )}
    </EditorSheet>
  );
}
