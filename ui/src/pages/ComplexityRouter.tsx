import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeftRight, Plus, Trash2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { PageBody, Pill } from "@/components/screen";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
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

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `routes` is the query the user is actually waiting on for this screen
  useScreenReady(!routes.isLoading);
  useErrorState(!!routes.error, "complexity-router");

  const policyQueries = useQueries({
    queries: (routes.data ?? []).map((r) => ({
      queryKey: ["route-complexity", r.id],
      queryFn: () => fetchRouteComplexity(r.id),
    })),
  });

  const [editing, setEditing] = React.useState<RouteRow | null>(null);
  // a policy lives on its route, so both entry points are `route:update`
  const updateGate = useGate("route:update");

  const withPolicy = (routes.data ?? []).map((r, i) => ({
    route: r,
    tiers: policyQueries[i]?.data?.tiers ?? [],
  }));
  const configured = withPolicy.filter((p) => p.tiers.length > 0);
  const unconfigured = withPolicy.filter((p) => p.tiers.length === 0);

  return (
    <PageBody>
      <span className="text-sm text-muted-foreground">
        {configured.length} of {withPolicy.length} routes have a complexity policy · requests are
        measured by input bytes and routed to the matching tier
      </span>

      {routes.isLoading && <CardGridSkeleton cards={3} height={196} min={380} />}
      {routes.error && (
        <LoadError
          error={routes.error}
          resource={t("errors.resources.routes")}
          onRetry={() => void routes.refetch()}
        />
      )}
      {!routes.isLoading && !routes.error && withPolicy.length === 0 && (
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
                {tiers.length} tiers
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
                      ? "catch-all"
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
                Edit policy
              </GatedButton>
            </div>
          </div>
        ))}
      </div>

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
      title="Complexity policy"
      subtitle={`${route.model} · tiers checked in order by input size`}
      dirty={dirty}
      errorMessage={save.isError ? (save.error as Error).message : undefined}
      saveLabel="Save"
      canSave={!!tiers && tiers.length > 0}
      saving={save.isPending}
      onSave={() => tiers && save.mutate(tiers)}
    >
      <div className="space-y-2.5">
        {(tiers ?? []).map((t, i) => (
          <div key={i} className="flex items-center gap-2">
            <Input
              className="w-[110px] font-mono text-xs"
              value={t.name}
              placeholder="tier name"
              onChange={(e) => set(i, { name: e.target.value })}
            />
            <Input
              className="w-[110px] font-mono text-xs"
              type="number"
              min={1}
              value={t.max_input_bytes ?? ""}
              placeholder="catch-all"
              onChange={(e) =>
                set(i, {
                  max_input_bytes: e.target.value === "" ? null : Number(e.target.value),
                })
              }
            />
            <Select
              className="min-w-0 flex-1 font-mono text-xs"
              aria-label={translate("pages.complexityRouter.tierRouteAria")}
              value={t.route}
              onChange={(e) => set(i, { route: e.target.value })}
            >
              {allRoutes.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </Select>
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
          Add tier
        </Button>
      </div>
    </EditorSheet>
  );
}
