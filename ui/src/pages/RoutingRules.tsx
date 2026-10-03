import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Route, Tag } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { ModelSheet } from "@/components/ModelSheet";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { Button } from "@/components/ui/button";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton, LoadingRegion } from "@/components/LoadingState";
import { PageBody, Toolbar } from "@/components/screen";
import { EmptyState } from "@/components/ui/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { LabelChips, LabelFilterSelect, LabelSheet, useSubjectLabels } from "@/components/Labels";
import {
  deleteRoute,
  fetchModels,
  fetchProviders,
  fetchRoutes,
  fetchRouteTargets,
  type RouteRow,
  type RouteTargetRow,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { strategyTone, usesWeights } from "@/lib/strategies";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const TARGET_BARS = [
  "var(--red-folk)",
  "var(--zinc-400)",
  "var(--status-info)",
  "var(--status-success)",
];

/**
 * What is known about one route's targets.
 *
 * `loading` and `failed` are the absence of an answer. Folding either into an
 * empty list said "No targets yet" and "0 targets" about a route whose targets
 * were never read (#2133), the mistake #1461 fixed for complexity policies.
 */
type TargetsState =
  | { kind: "loading" }
  | { kind: "failed"; error: unknown; retry: () => void }
  | { kind: "loaded"; targets: RouteTargetRow[] };

// routing rules: one card per route with its strategy pill, its targets (and
// their weight shares where the strategy reads weights), and label/delete
// actions
export default function RoutingRules() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const scope = useScope();
  // deleting a route is the same admin capability adding one is (#1258)

  const routes = useQuery({
    queryKey: ["routes", scope.projectId],
    queryFn: () => fetchRoutes(scope.projectId as string),
    enabled: !!scope.projectId,
  });

  const providers = useQuery({
    queryKey: ["providers", scope.orgId],
    queryFn: () => fetchProviders(scope.orgId as string),
    enabled: !!scope.orgId,
  });

  const targetQueries = useQueries({
    queries: (routes.data ?? []).map((r) => ({
      queryKey: ["route-targets", r.id],
      queryFn: () => fetchRouteTargets(r.id),
    })),
  });
  // one read per route, so each card says which answer it holds. pending
  // rather than loading: a retry parked in a hidden tab is still not an answer
  const targetsByRoute = new Map<string, TargetsState>();
  (routes.data ?? []).forEach((r, i) => {
    const query = targetQueries[i];
    let state: TargetsState;
    if (!query || query.isPending) state = { kind: "loading" };
    else if (query.error) {
      state = { kind: "failed", error: query.error, retry: () => void query.refetch() };
    } else state = { kind: "loaded", targets: query.data ?? [] };
    targetsByRoute.set(r.id, state);
  });
  const targetStates = [...targetsByRoute.values()];

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider.
  // the route list is what the reader waits on first and the target reads
  // decide what every card says, so readiness and the error signal follow both
  useScreenReady(!routes.isLoading && !targetStates.some((s) => s.kind === "loading"));
  useErrorState(!!routes.error, "routing-rules");
  useErrorState(
    targetStates.some((s) => s.kind === "failed"),
    "route-targets",
  );

  const providerName = (id: string) =>
    providers.data?.find((p) => p.id === id)?.name ?? id.slice(0, 8);

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["routes", scope.projectId] });
    queryClient.invalidateQueries({ queryKey: ["models"] });
    queryClient.invalidateQueries({ queryKey: ["config"] });
  };

  const remove = useMutation({
    mutationFn: (id: string) => deleteRoute(id),
    onSuccess: invalidate,
  });

  // a route is created through the model sheet, the same one Model Catalog
  // opens, so the strategy, the targets and everything else about a new model
  // are set in one place rather than in two forms that disagreed (#1979). the
  // catalog is read only for the sheet's name-conflict check
  const [addOpen, setAddOpen] = React.useState(false);
  const models = useQuery({ queryKey: ["models"], queryFn: fetchModels, enabled: addOpen });
  // a route is the public name clients call; deleting one breaks them silently,
  // so it is confirmed by name before anything leaves (#1179)
  const [deleteTarget, setDeleteTarget] = React.useState<RouteRow | null>(null);
  const [labelFilter, setLabelFilter] = React.useState("");
  const [labelling, setLabelling] = React.useState<RouteRow | null>(null);
  // routes are project-scoped rows, but a label lives on the org that owns them
  const labels = useSubjectLabels(scope.orgId, "route");
  // reset first: an error left over from a previous failed delete would
  // otherwise greet the next route the operator picks
  const startDelete = (route: RouteRow) => {
    remove.reset();
    setDeleteTarget(route);
  };

  const shown = (routes.data ?? []).filter((r) => labels.matches(r.id, labelFilter));

  return (
    <PageBody>
      <Toolbar>
        {/* a count needs an answer: "0 routes" while the list was still out,
            or under a 403, stated an empty project nobody had seen (#2133) */}
        {routes.isSuccess && (
          <span className="text-sm text-muted-foreground">
            {t("pages.routing.summary", { count: routes.data.length })}
          </span>
        )}
        <LabelFilterSelect value={labelFilter} onChange={setLabelFilter} options={labels.options} />
        <GatedButton
          gate="route:create"
          control="route-new"
          className="ml-auto"
          onClick={() => setAddOpen(true)}
          disabled={!scope.projectId}
        >
          + {t("pages.routing.emptyAction")}
        </GatedButton>
      </Toolbar>

      {/* pending, not loading, so a retry parked in a hidden tab keeps the
          skeleton; the project guard keeps it off a scope with no project,
          where the query never runs */}
      {routes.isPending && !!scope.projectId && (
        <CardGridSkeleton cards={3} height={196} min={360} />
      )}
      {routes.error && (
        <LoadError
          error={routes.error}
          resource={t("errors.resources.routes")}
          onRetry={() => void routes.refetch()}
        />
      )}
      {routes.isSuccess && shown.length === 0 && (
        // a label filter that matches nothing is not a project with no routes:
        // the copy blames the narrowing and offers to clear it rather than
        // offering to add the first route to a project that already has some
        <EmptyState
          uxTarget="routes"
          icon={<Route />}
          title={labelFilter ? t("pages.routing.noMatchTitle") : t("pages.routing.emptyTitle")}
          description={labelFilter ? t("pages.routing.noMatchBody") : t("pages.routing.emptyBody")}
          actions={
            labelFilter ? (
              <Button variant="outline" onClick={() => setLabelFilter("")}>
                {t("common.clearFilters")}
              </Button>
            ) : (
              <GatedButton
                gate="route:create"
                control="route-new-empty"
                disabled={!scope.projectId}
                onClick={() => setAddOpen(true)}
              >
                {t("pages.routing.emptyAction")}
              </GatedButton>
            )
          }
        />
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(360px,100%),1fr))]">
        {shown.map((r) => {
          const targets: TargetsState = targetsByRoute.get(r.id) ?? { kind: "loading" };
          const tone = strategyTone(r.strategy);
          return (
            <div
              key={r.id}
              className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4"
            >
              <div className="flex items-center gap-2.5">
                <span className="min-w-0 truncate font-mono text-sm font-semibold">{r.model}</span>
                <span
                  className="whitespace-nowrap rounded-[6px] px-2 py-[3px] font-mono text-[0.6875rem] uppercase tracking-[0.04em]"
                  style={{ color: tone[0], background: tone[1] }}
                >
                  {r.strategy}
                </span>
                <LabelChips labels={labels.bySubject(r.id)} />
                {!r.enabled && (
                  <span className="rounded-[6px] bg-[color:var(--surface-subtle)] px-2 py-[3px] font-mono text-[0.6875rem] uppercase text-[color:var(--text-subtle)]">
                    {t("pages.routing.disabledBadge")}
                  </span>
                )}
              </div>
              <RouteTargets route={r} state={targets} providerName={providerName} />
              <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                {/* the count needs an answer too: a pending or failed read is
                    not a route with no targets */}
                {targets.kind === "loaded" && (
                  <span className="text-xs text-[color:var(--text-subtle)]">
                    {t("pages.routing.targetCount", { count: targets.targets.length })}
                  </span>
                )}
                {/* the label names the route, so a grid of cards does not
                    expose N buttons a screen reader cannot tell apart (#1214) */}
                <Button
                  size="sm"
                  variant="outline"
                  className="ml-auto h-[30px]"
                  aria-label={t("labels.labelsOf", { name: r.model })}
                  onClick={() => setLabelling(r)}
                >
                  <Tag className="h-3.5 w-3.5" />
                </Button>
                <DeleteIconButton
                  gate="route:delete"
                  control="route-delete"
                  label={t("pages.routing.deleteRoute", { model: r.model })}
                  pending={remove.isPending && remove.variables === r.id}
                  onClick={() => startDelete(r)}
                />
              </div>
            </div>
          );
        })}
      </div>
      {remove.isError && !deleteTarget && (
        <p className="text-xs text-[color:var(--status-danger-text)]">
          {(remove.error as Error).message}
        </p>
      )}

      {scope.orgId && labelling && (
        <LabelSheet
          open
          onOpenChange={(open) => !open && setLabelling(null)}
          orgId={scope.orgId}
          subjectType="route"
          subjectId={labelling.id}
          subjectName={labelling.model}
        />
      )}

      <ConfirmDialog
        name="route-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("pages.routing.confirm.title", { model: deleteTarget?.model })}
        description={t("pages.routing.confirm.body")}
        confirmLabel={t("pages.routing.confirm.confirm")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => {
          if (!deleteTarget) return;
          const what = deleteTarget.model;
          remove.mutate(deleteTarget.id, {
            onSuccess: () => {
              setDeleteTarget(null);
              toast.push({ tone: "success", title: t("toast.deleted", { what }) });
            },
            onError: (error) => {
              toast.push({
                tone: "error",
                title: t("toast.deleteFailed", { what }),
                detail: errorDetail(error),
              });
            },
          });
        }}
      />

      <ModelSheet
        open={addOpen}
        mode="add"
        onOpenChange={setAddOpen}
        projectId={scope.projectId ?? null}
        orgId={scope.orgId ?? null}
        providers={providers.data ?? []}
        models={models.data ?? []}
        routes={routes.data ?? []}
        onDone={invalidate}
      />
    </PageBody>
  );
}

/**
 * The target half of a route card, for whichever answer the card holds.
 *
 * No health is drawn. Nothing on this screen reads it, and the dot that stood
 * here was always green, so a target behind an open breaker read as healthy
 * (#2133). A share of the route is drawn only under a strategy that reads
 * weights; under any other the balancer never consults them, so the card says
 * that once instead of printing a split the gateway does not make.
 */
function RouteTargets({
  route,
  state,
  providerName,
}: {
  route: RouteRow;
  state: TargetsState;
  providerName: (id: string) => string;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const weighted = usesWeights(route.strategy);

  if (state.kind === "loading") {
    // shaped like the rows it stands in for, so the card holds its height
    return (
      <LoadingRegion className="flex flex-col gap-2.5" testId="route-targets-loading">
        {[72, 56].map((width) => (
          <div key={width} className="flex flex-col gap-[5px]">
            <Skeleton width={`${width}%`} height={16} radius={4} />
            {weighted && <Skeleton height={5} radius={9999} />}
          </div>
        ))}
      </LoadingRegion>
    );
  }
  if (state.kind === "failed") {
    return (
      <LoadError
        error={state.error}
        resource={t("errors.resources.routeTargets")}
        onRetry={state.retry}
      />
    );
  }

  const { targets } = state;
  if (targets.length === 0) {
    return <p className="text-xs text-muted-foreground">{t("pages.routing.noTargets")}</p>;
  }
  const totalWeight = targets.reduce((a, tg) => a + tg.weight, 0) || 1;
  return (
    <div className="flex flex-col gap-2.5">
      {!weighted && targets.length > 1 && (
        <p className="text-xs leading-snug text-muted-foreground">
          <Trans
            i18nKey="routeTargets.weightsIgnored"
            values={{ strategy: route.strategy }}
            components={[<span key="strategy" className="font-mono text-foreground" />]}
          />
        </p>
      )}
      <ul
        aria-label={t("routeTargets.listLabel", { model: route.model })}
        className="flex flex-col gap-2.5"
      >
        {targets.map((tg, i) => {
          const share = tg.weight / totalWeight;
          return (
            <li key={tg.id} className="flex flex-col gap-[5px]">
              <div className="flex items-center gap-2 font-mono text-xs">
                <span className="text-[color:var(--text-secondary)]">
                  {providerName(tg.provider_id)}
                </span>
                <span className="text-[color:var(--text-subtle)]">→</span>
                <span className="min-w-0 truncate text-muted-foreground">
                  {tg.upstream_model || route.model}
                </span>
                {weighted && (
                  <span className="ml-auto text-[color:var(--text-secondary)]">
                    {fmt.percent(share, 0)}
                  </span>
                )}
              </div>
              {weighted && (
                <div
                  data-testid="target-share"
                  className="h-[5px] overflow-hidden rounded-full bg-[color:var(--surface-subtle)]"
                >
                  <div
                    className="h-full rounded-full"
                    style={{
                      // a CSS length, never a localized percentage
                      width: `${Math.round(share * 100)}%`,
                      background: TARGET_BARS[i % TARGET_BARS.length],
                    }}
                  />
                </div>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
