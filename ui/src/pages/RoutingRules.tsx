import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Route, Trash2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { PageBody, StatusDot, Toolbar } from "@/components/screen";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import {
  createRoute,
  createRouteTarget,
  deleteRoute,
  fetchProviders,
  fetchRoutes,
  fetchRouteTargets,
  STRATEGIES,
  type RouteRow,
  type RouteTargetRow,
} from "@/lib/api";
import { StrategyHint } from "@/components/StrategyHint";
import { useGate } from "@/lib/can";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { strategyOptions, strategyTone } from "@/lib/strategies";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const TARGET_BARS = ["var(--red-folk)", "var(--zinc-400)", "var(--status-info)", "var(--status-success)"];

// routing rules from the design prototype: one card per route with its
// strategy pill, per-target weight bars, and edit/delete actions
export default function RoutingRules() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const queryClient = useQueryClient();
  const toast = useToast();
  const scope = useScope();
  // deleting a route is the same admin capability adding one is (#1258)
  const deleteGate = useGate("route:delete");

  const routes = useQuery({
    queryKey: ["routes", scope.projectId],
    queryFn: () => fetchRoutes(scope.projectId as string),
    enabled: !!scope.projectId,
  });


  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `routes` is the query the user is actually waiting on for this screen

  useScreenReady(!routes.isLoading);

  useErrorState(!!routes.error, "routing-rules");
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
  const targetsByRoute = new Map<string, RouteTargetRow[]>();
  (routes.data ?? []).forEach((r, i) => {
    targetsByRoute.set(r.id, targetQueries[i]?.data ?? []);
  });

  const providerName = (id: string) =>
    providers.data?.find((p) => p.id === id)?.name ?? id.slice(0, 8);

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["routes", scope.projectId] });
    queryClient.invalidateQueries({ queryKey: ["models"] });
  };

  const remove = useMutation({
    mutationFn: (id: string) => deleteRoute(id),
    onSuccess: invalidate,
  });

  const [addOpen, setAddOpen] = React.useState(false);
  // a route is the public name clients call; deleting one breaks them silently,
  // so it is confirmed by name before anything leaves (#1179)
  const [deleteTarget, setDeleteTarget] = React.useState<RouteRow | null>(null);
  // reset first: an error left over from a previous failed delete would
  // otherwise greet the next route the operator picks
  const startDelete = (route: RouteRow) => {
    remove.reset();
    setDeleteTarget(route);
  };

  return (
    <PageBody>
      <Toolbar>
        <span className="text-sm text-muted-foreground">
          {routes.data?.length ?? 0} routes · public model names clients call, resolved to
          upstream targets
        </span>
        <GatedButton gate="route:create" className="ml-auto" onClick={() => setAddOpen(true)} disabled={!scope.projectId}>
          + Add route
        </GatedButton>
      </Toolbar>

      {routes.isLoading && <CardGridSkeleton cards={3} height={196} min={360} />}
      {routes.error && (
        <LoadError
          error={routes.error}
          resource={t("errors.resources.routes")}
          onRetry={() => void routes.refetch()}
        />
      )}
      {routes.data && routes.data.length === 0 && (
        <EmptyState
          uxTarget="routes"
          icon={<Route />}
          title={t("pages.routing.emptyTitle")}
          description={t("pages.routing.emptyBody")}
          actions={
            <GatedButton gate="route:create" disabled={!scope.projectId} onClick={() => setAddOpen(true)}>
              {t("pages.routing.emptyAction")}
            </GatedButton>
          }
        />
      )}
      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(360px,100%),1fr))]">
        {(routes.data ?? []).map((r) => {
          const targets = targetsByRoute.get(r.id) ?? [];
          const totalWeight = targets.reduce((a, t) => a + t.weight, 0) || 1;
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
                {!r.enabled && (
                  <span className="rounded-[6px] bg-[color:var(--surface-subtle)] px-2 py-[3px] font-mono text-[0.6875rem] uppercase text-[color:var(--text-subtle)]">
                    disabled
                  </span>
                )}
              </div>
              <div className="flex flex-col gap-2.5">
                {targets.length === 0 && (
                  <p className="text-xs text-muted-foreground">No targets yet.</p>
                )}
                {targets.map((t, i) => {
                  const share = t.weight / totalWeight;
                  return (
                    <div key={t.id} className="flex flex-col gap-[5px]">
                      <div className="flex items-center gap-2 font-mono text-xs">
                        <StatusDot color="var(--status-success)" className="h-1.5 w-1.5" />
                        <span className="text-[color:var(--text-secondary)]">
                          {providerName(t.provider_id)}
                        </span>
                        <span className="text-[color:var(--text-subtle)]">→</span>
                        <span className="min-w-0 truncate text-muted-foreground">
                          {t.upstream_model || r.model}
                        </span>
                        <span className="ml-auto text-[color:var(--text-secondary)]">
                          {fmt.percent(share, 0)}
                        </span>
                      </div>
                      <div className="h-[5px] overflow-hidden rounded-full bg-[color:var(--surface-subtle)]">
                        <div
                          className="h-full rounded-full"
                          style={{
                            // a CSS length, never a localized percentage
                            width: `${Math.round(share * 100)}%`,
                            background: TARGET_BARS[i % TARGET_BARS.length],
                          }}
                        />
                      </div>
                    </div>
                  );
                })}
              </div>
              <div className="flex items-center gap-2 border-t border-[color:var(--border-subtle)] pt-3">
                <span className="text-xs text-[color:var(--text-subtle)]">
                  {t("pages.routing.targetCount", { count: targets.length })}
                </span>
                {/* the label names the route, so a grid of cards does not
                    expose N buttons a screen reader cannot tell apart (#1214) */}
                <button
                  type="button"
                  title={
                    deleteGate.reason ??
                    t("pages.routing.deleteRoute", { model: r.model })
                  }
                  aria-label={t("pages.routing.deleteRoute", { model: r.model })}
                  disabled={
                    deleteGate.denied ||
                    (remove.isPending && remove.variables === r.id)
                  }
                  onClick={() => startDelete(r)}
                  className="ml-auto flex h-[30px] items-center rounded-[6px] border border-[color:var(--border-subtle)] px-2 text-[color:var(--status-danger-text)] transition-colors hover:bg-[color:var(--red-tint)] disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  {remove.isPending && remove.variables === r.id ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <Trash2 className="h-3.5 w-3.5" />
                  )}
                </button>
              </div>
            </div>
          );
        })}
      </div>
      {remove.isError && !deleteTarget && (
        <p className="text-xs text-[color:var(--status-danger-text)]">{(remove.error as Error).message}</p>
      )}

      <ConfirmDialog
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

      {scope.projectId && (
        <AddRouteDialog
          open={addOpen}
          onOpenChange={setAddOpen}
          projectId={scope.projectId}
          providers={providers.data?.map((p) => ({ id: p.id, name: p.name })) ?? []}
          onDone={invalidate}
        />
      )}
    </PageBody>
  );
}

function AddRouteDialog({
  open,
  onOpenChange,
  projectId,
  providers,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  projectId: string;
  providers: { id: string; name: string }[];
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const [model, setModel] = React.useState("");
  const [strategy, setStrategy] = React.useState<string>(STRATEGIES[0]);
  const [providerId, setProviderId] = React.useState("");
  const [weight, setWeight] = React.useState("100");

  React.useEffect(() => {
    if (open) {
      setModel("");
      setStrategy(STRATEGIES[0]);
      setProviderId(providers[0]?.id ?? "");
      setWeight("100");
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const create = useMutation({
    mutationFn: async () => {
      const route = await createRoute(projectId, { model, strategy });
      if (providerId) {
        await createRouteTarget(route.id, {
          provider_id: providerId,
          weight: Number(weight) || 1,
        });
      }
    },
    onSuccess: () => {
      // the dialog closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({ tone: "success", title: t("toast.created", { what: model }) });
      onDone();
      onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: model }),
        detail: errorDetail(error),
      });
    },
  });

  const dirty = !!(model.trim() || strategy !== STRATEGIES[0] || weight !== "100");

  return (
    <EditorSheet
      open={open}
      onOpenChange={onOpenChange}
      title="Add route"
      subtitle="Public model name resolved to upstream targets by the chosen strategy."
      dirty={dirty}
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      saveLabel="Create"
      canSave={!!model.trim()}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
      <div className="space-y-3">
        <Field label="Model name">
          <Input
            className="font-mono"
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="gpt-4o"
          />
        </Field>
        {/* the select plus its caveat, so the label is bound by hand (#1264) */}
        <Field label="Strategy" htmlFor="route-strategy">
          <Select
            id="route-strategy"
            value={strategy}
            onChange={(e) => setStrategy(e.target.value)}
          >
            {strategyOptions(strategy).map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </Select>
          <StrategyHint strategy={strategy} />
        </Field>
        <Field label="First target">
          <Select value={providerId} onChange={(e) => setProviderId(e.target.value)}>
            <option value="">none (attach later)</option>
            {providers.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </Select>
        </Field>
        {providerId && (
          <Field label="Weight">
            <Input
              type="number"
              min={1}
              value={weight}
              onChange={(e) => setWeight(e.target.value)}
            />
          </Field>
        )}
      </div>
    </EditorSheet>
  );
}
