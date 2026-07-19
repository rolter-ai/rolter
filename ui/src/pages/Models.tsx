import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Lock, Plus, Trash2 } from "lucide-react";
import * as React from "react";
import { useNavigate } from "react-router-dom";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Table } from "@/components/ui/table";
import { Tabs } from "@/components/ui/tabs";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { ModelSheet, type ModelSheetMode } from "@/components/ModelSheet";
import {
  deleteModel,
  fetchModels,
  fetchProviders,
  fetchRoutes,
  setRouteEnabled,
  type EffectiveModelDto,
  type ProviderRow,
  type RouteRow,
} from "@/lib/api";
import { useScope } from "@/lib/scope";

const MODELS_QUERY_KEY = ["models"];

export default function Models() {
  const queryClient = useQueryClient();
  const scope = useScope();
  const navigate = useNavigate();
  const [tab, setTab] = React.useState("routes");

  const models = useQuery({
    queryKey: MODELS_QUERY_KEY,
    queryFn: fetchModels,
  });

  // db-owned routes give us the route id + live enabled state the effective
  // model list doesn't carry; only fetched once we know which project owns them
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

  const routeByModel = React.useMemo(() => {
    const map = new Map<string, RouteRow>();
    for (const route of routes.data ?? []) {
      map.set(route.model, route);
    }
    return map;
  }, [routes.data]);

  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: MODELS_QUERY_KEY });
    queryClient.invalidateQueries({ queryKey: ["routes", scope.projectId] });
  };

  const toggleEnabled = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      setRouteEnabled(id, enabled),
    onSuccess: invalidate,
  });

  const removeModel = useMutation({
    mutationFn: (model: string) => deleteModel(model),
    onSuccess: invalidate,
  });

  // one unified add/edit/view slide-over sheet replaces the old thin dialogs
  const [sheet, setSheet] = React.useState<{
    mode: ModelSheetMode;
    route?: RouteRow | null;
    configModel?: EffectiveModelDto | null;
  } | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<EffectiveModelDto | null>(
    null,
  );

  const scopeBlocked = !scope.isLoading && !!scope.error;

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold">Models</h1>
          <p className="text-sm text-muted-foreground">
            Public model names routed by rolter.
          </p>
        </div>
        {tab === "routes" ? (
          <Button
            size="sm"
            onClick={() => setSheet({ mode: "add" })}
            disabled={scopeBlocked || !scope.projectId}
          >
            <Plus className="h-4 w-4" />
            Add model
          </Button>
        ) : (
          <Button size="sm" variant="outline" onClick={() => navigate("/providers")}>
            Manage providers
          </Button>
        )}
      </div>

      <Tabs
        value={tab}
        onChange={setTab}
        tabs={[
          { value: "routes", label: "Routes", count: models.data?.length },
          { value: "providers", label: "Providers", count: providers.data?.length },
        ]}
      />

      {tab === "providers" ? (
        <ProvidersTab providers={providers.data ?? []} loading={providers.isLoading} />
      ) : (
        <>
      {models.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {models.error && (
        <p className="text-sm text-destructive">Failed to load models.</p>
      )}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          Add/edit/delete is unavailable: {scope.error}. Read-only view still works.
        </p>
      )}

      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {models.data?.map((entry) => {
          const route = routeByModel.get(entry.model);
          const isConfigOwned = entry.source === "config";
          return (
            <Card key={entry.model}>
              <CardHeader>
                <CardTitle className="flex items-center justify-between gap-2">
                  <span className="truncate">{entry.model}</span>
                  {isConfigOwned ? (
                    <span title="managed by the bootstrap config file">
                      <Lock className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                    </span>
                  ) : (
                    <Switch
                      checked={route?.enabled ?? true}
                      disabled={!route || toggleEnabled.isPending}
                      onCheckedChange={(enabled) =>
                        route && toggleEnabled.mutate({ id: route.id, enabled })
                      }
                    />
                  )}
                </CardTitle>
                <CardDescription className="flex items-center gap-2">
                  <Badge tone="outline">{entry.strategy}</Badge>
                  <Badge tone={isConfigOwned ? "neutral" : "info"}>
                    {entry.source}
                  </Badge>
                  {entry.targets} target(s)
                </CardDescription>
              </CardHeader>
              <CardContent className="flex items-center justify-between gap-2">
                <p className="text-sm text-muted-foreground">
                  {isConfigOwned
                    ? "edit the config file and restart to change this model"
                    : "db-managed"}
                </p>
                {isConfigOwned ? (
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => setSheet({ mode: "view", configModel: entry })}
                  >
                    View
                  </Button>
                ) : (
                  <div className="flex shrink-0 gap-1">
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={!route}
                      onClick={() => route && setSheet({ mode: "edit", route })}
                    >
                      Edit
                    </Button>
                    <Button
                      size="sm"
                      variant="destructive"
                      onClick={() => setDeleteTarget(entry)}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                )}
              </CardContent>
            </Card>
          );
        })}
      </div>
        </>
      )}

      <ModelSheet
        open={!!sheet}
        mode={sheet?.mode ?? "add"}
        onOpenChange={(open) => !open && setSheet(null)}
        projectId={scope.projectId ?? null}
        orgId={scope.orgId ?? null}
        providers={providers.data ?? []}
        route={sheet?.route ?? null}
        configModel={sheet?.configModel ?? null}
        models={models.data ?? []}
        routes={routes.data ?? []}
        onDone={invalidate}
      />

      <Dialog open={!!deleteTarget} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <DialogHeader>
          <DialogTitle>Delete model</DialogTitle>
          <DialogDescription>
            This removes all routes and targets for{" "}
            <span className="font-mono">{deleteTarget?.model}</span>. This cannot be
            undone.
          </DialogDescription>
        </DialogHeader>
        {removeModel.isError && (
          <p className="text-xs text-destructive">
            {(removeModel.error as Error).message}
          </p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => setDeleteTarget(null)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={removeModel.isPending}
            onClick={() => {
              if (!deleteTarget) return;
              removeModel.mutate(deleteTarget.model, {
                onSuccess: () => setDeleteTarget(null),
              });
            }}
          >
            Delete
          </Button>
        </DialogFooter>
      </Dialog>
    </div>
  );
}

// read-only providers view for the Models "Providers" tab (DS parity). full
// CRUD lives on the Providers admin page — linked from the header.
function ProvidersTab({
  providers,
  loading,
}: {
  providers: ProviderRow[];
  loading: boolean;
}) {
  if (loading) {
    return <p className="text-sm text-muted-foreground">Loading…</p>;
  }
  if (providers.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        No providers configured for this org yet.
      </p>
    );
  }
  return (
    <Table
      rowKey="id"
      columns={[
        { key: "name", header: "Provider", mono: true },
        {
          key: "kind",
          header: "Kind",
          render: (v) => <Badge tone="neutral">{v as string}</Badge>,
        },
        { key: "slug", header: "Slug", mono: true },
        { key: "api_base", header: "API base", mono: true },
        {
          key: "api_key_env",
          header: "Key env",
          mono: true,
          render: (v) => (v as string) || "—",
        },
      ]}
      data={providers as unknown as Record<string, unknown>[]}
    />
  );
}
