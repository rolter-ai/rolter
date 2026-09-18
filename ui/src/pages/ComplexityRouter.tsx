import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeftRight, CircleHelp, Plus, Trash2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton, FormSkeleton } from "@/components/LoadingState";
import { PageBody, Pill } from "@/components/screen";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import {
  fetchRouteComplexity,
  fetchRoutes,
  setRouteComplexity,
  type ComplexityTier,
  type RouteRow,
} from "@/lib/api";
import { useGate } from "@/lib/can";
import { useFormat, type Formatters } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

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
  return (state: PolicyState): state is Extract<PolicyState, { kind: K }> =>
    state.kind === kind;
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
  // a policy lives on its route, so both entry points are `route:update`
  const updateGate = useGate("route:update");

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
  // what every card says — so readiness and the error signal follow both
  useScreenReady(!routes.isLoading && checking.length === 0);
  useErrorState(!!routes.error, "complexity-router");
  useErrorState(failed.length > 0, "complexity-policies");

  return (
    <PageBody>
      <span className="text-sm text-muted-foreground">
        {t("pages.complexityRouter.summary", {
          configured: configured.length,
          count: resolved,
        })}
      </span>
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
        />
      )}
      {!routes.isLoading && !routes.error && states.length === 0 && (
        // a complexity policy hangs off a route, so with no routes there is
        // nothing on this screen to create — the CTA points where it is made
        <EmptyState
          uxTarget="complexity-routes"
          icon={<ArrowLeftRight />}
          title={t("pages.complexityRouter.emptyTitle")}
          description={t("pages.complexityRouter.emptyBody")}
          actions={
            <a
              href="/routing-rules"
              className="text-sm font-medium text-foreground underline decoration-[color:var(--border-strong)] underline-offset-4 transition-colors hover:decoration-current focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              {t("pages.complexityRouter.emptyAction")}
            </a>
          }
        />
      )}
      {failed.length > 0 && (
        <div className="flex flex-col gap-2.5">
          <div className="text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]">
            {t("pages.complexityRouter.policyLoadFailed")}
          </div>
          {/* one alert for the group rather than one per route: it is the same
              endpoint failing every time, and the routes it covers are named
              underneath. retry re-runs exactly the reads that failed */}
          <LoadError
            error={failed[0].error}
            resource={t("errors.resources.complexityPolicies")}
            onRetry={() => failed.forEach((f) => f.retry())}
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
          <div
            key={route.id}
            className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
          >
            <div className="flex items-center gap-2.5">
              <span className="flex h-[34px] w-[34px] flex-none items-center justify-center rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-[color:var(--red-folk-text)]">
                <ArrowLeftRight className="h-4 w-4" />
              </span>
              <span className="min-w-0 truncate font-mono text-sm font-semibold">
                {route.model}
              </span>
              <Pill
                className="ml-auto"
                color="var(--status-info-text)"
                tint="rgba(59,130,246,.14)"
              >
                {t("pages.complexityRouter.tierCount", { count: tiers.length })}
              </Pill>
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
            <div className="flex items-center justify-end border-t border-[color:var(--border-subtle)] pt-3">
              {/* a complexity policy is stored on the route, so editing one
                  is the route's own update capability (#1258) */}
              <GatedButton
                gate="route:update"
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
          </div>
        ))}
      </div>

      {!routes.isLoading && checking.length > 0 && (
        <div className="flex flex-col gap-2.5">
          <div className="text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]">
            {t("pages.complexityRouter.checkingPolicy")}
          </div>
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
          <div className="mt-2 text-[0.6875rem] uppercase tracking-[0.07em] text-[color:var(--text-subtle)]">
            {t("pages.complexityRouter.noPolicyYet")}
          </div>
          <div className="flex flex-wrap gap-2.5">
            {unconfigured.map(({ route }) => (
              <button
                key={route.id}
                type="button"
                title={updateGate.reason}
                aria-label={t("pages.complexityRouter.addPolicyAria", {
                  model: route.model,
                })}
                disabled={updateGate.denied}
                onClick={() => setEditing(route)}
                className="flex items-center gap-2 rounded-[8px] border border-[color:var(--border-subtle)] px-3 py-2 font-mono text-xs text-[color:var(--text-secondary)] transition-colors hover:border-[color:var(--border-default)] hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {route.model}
                <Plus className="h-3 w-3" />
              </button>
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
        detail: errorDetail(error),
      });
    },
  });
  const set = (i: number, patch: Partial<ComplexityTier>) =>
    setTiers((ts) => ts?.map((t, j) => (j === i ? { ...t, ...patch } : t)) ?? null);

  const dirty = JSON.stringify(tiers) !== JSON.stringify(existing.data?.tiers ?? null);

  return (
    <EditorSheet
      open
      onOpenChange={(open) => !open && onClose()}
      title={translate("pages.complexityRouter.policyTitle")}
      subtitle={translate("pages.complexityRouter.policySubtitle", {
        model: route.model,
      })}
      dirty={dirty}
      errorMessage={save.isError ? (save.error as Error).message : undefined}
      saveLabel={translate("common.save")}
      canSave={!!tiers && tiers.length > 0 && !existing.isPending && !existing.error}
      saving={save.isPending}
      onSave={() => tiers && save.mutate(tiers)}
    >
      {/* the editor seeds itself from the same read the list makes, so it holds
          nothing in the same two ways: still waiting, and failed. it rendered
          the default two-tier draft for both, offering a save that would have
          replaced a policy nobody had seen (#1461) */}
      {existing.isPending && <FormSkeleton fields={3} />}
      {existing.error && (
        <LoadError
          error={existing.error}
          resource={translate("errors.resources.complexityPolicy")}
          onRetry={() => void existing.refetch()}
        />
      )}
      {!existing.isPending && !existing.error && (
        <div className="space-y-2.5">
          {(tiers ?? []).map((t, i) => (
            <div key={i} className="flex items-center gap-2">
              <Input
                className="w-[110px] font-mono text-xs"
                value={t.name}
                placeholder={translate("pages.complexityRouter.tierNamePlaceholder")}
                onChange={(e) => set(i, { name: e.target.value })}
              />
              <Input
                className="w-[110px] font-mono text-xs"
                type="number"
                min={1}
                value={t.max_input_bytes ?? ""}
                placeholder={translate("pages.complexityRouter.catchAll")}
                onChange={(e) =>
                  set(i, {
                    max_input_bytes: e.target.value === "" ? null : Number(e.target.value),
                  })
                }
              />
              <Combobox
                size="sm"
                className="min-w-0 flex-1 font-mono"
                aria-label={translate("pages.complexityRouter.tierRouteAria")}
                value={t.route}
                onChange={(route) => set(i, { route })}
                options={allRoutes.map((m) => ({ value: m, label: m }))}
              />
              <button
                type="button"
                title={translate("pages.complexityRouter.removeTierAria", {
                  name: t.name || i + 1,
                })}
                aria-label={translate("pages.complexityRouter.removeTierAria", {
                  name: t.name || i + 1,
                })}
                onClick={() => setTiers((ts) => ts?.filter((_, j) => j !== i) ?? null)}
                className="flex h-8 flex-none items-center rounded-[6px] border border-[color:var(--border-subtle)] px-2 text-[color:var(--status-danger-text)] transition-colors hover:bg-[color:var(--red-tint)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          ))}
          <Button
            size="sm"
            variant="outline"
            onClick={() =>
              setTiers((ts) => [
                ...(ts ?? []),
                { name: `tier-${(ts?.length ?? 0) + 1}`, max_input_bytes: null, route: route.model },
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
