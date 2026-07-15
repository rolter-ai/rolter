import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Lock, Plus, Trash2 } from "lucide-react";
import * as React from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import { CopyButton } from "@/components/CopyButton";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { AddProviderDialog } from "@/pages/Providers";
import {
  createRoute,
  createRouteTarget,
  deleteModel,
  deleteRoute,
  deleteRouteTarget,
  fetchModels,
  fetchProviders,
  fetchRoutes,
  fetchRouteTargets,
  setRouteEnabled,
  STRATEGIES,
  updateRouteParams,
  type EffectiveModelDto,
  type ProviderRow,
  type RouteRow,
} from "@/lib/api";
import { useScope } from "@/lib/scope";

const MODELS_QUERY_KEY = ["models"];

export default function Models() {
  const queryClient = useQueryClient();
  const scope = useScope();

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

  const [addOpen, setAddOpen] = React.useState(false);
  const [editModel, setEditModel] = React.useState<RouteRow | null>(null);
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
        <Button
          size="sm"
          onClick={() => setAddOpen(true)}
          disabled={scopeBlocked || !scope.projectId}
        >
          <Plus className="h-4 w-4" />
          Add model
        </Button>
      </div>

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
                {!isConfigOwned && (
                  <div className="flex shrink-0 gap-1">
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={!route}
                      onClick={() => route && setEditModel(route)}
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

      {scope.projectId && (
        <AddModelDialog
          open={addOpen}
          onOpenChange={setAddOpen}
          projectId={scope.projectId}
          orgId={scope.orgId ?? null}
          providers={providers.data ?? []}
          onProvidersChanged={() =>
            queryClient.invalidateQueries({ queryKey: ["providers", scope.orgId] })
          }
          onDone={invalidate}
        />
      )}

      <EditModelDialog
        route={editModel}
        onOpenChange={(open) => !open && setEditModel(null)}
        providers={providers.data ?? []}
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

function AddModelDialog({
  open,
  onOpenChange,
  projectId,
  orgId,
  providers,
  onProvidersChanged,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  projectId: string;
  orgId: string | null;
  providers: ProviderRow[];
  onProvidersChanged: () => void;
  onDone: () => void;
}) {
  const [model, setModel] = React.useState("");
  const [strategy, setStrategy] = React.useState<string>(STRATEGIES[0]);
  const [providerId, setProviderId] = React.useState("");
  const [upstreamModel, setUpstreamModel] = React.useState("");
  const [weight, setWeight] = React.useState("1");
  const [newProviderOpen, setNewProviderOpen] = React.useState(false);

  React.useEffect(() => {
    if (open) {
      setModel("");
      setStrategy(STRATEGIES[0]);
      setProviderId(providers[0]?.id ?? "");
      setUpstreamModel("");
      setWeight("1");
    }
  }, [open, providers]);

  const selectedProvider = providers.find((p) => p.id === providerId) ?? null;
  // the resolvable provider-slug/model address for the picked binding: the
  // upstream model (or the public model name when left blank) under the
  // provider's slug. this is exactly what a client can send as `model`
  const address =
    selectedProvider && (upstreamModel.trim() || model.trim())
      ? `${selectedProvider.slug}/${upstreamModel.trim() || model.trim()}`
      : null;

  // create-then-attach-target: two calls against the control api since a
  // route and its first target are separate resources. multi-target add on
  // creation is deferred — add further targets via edit after creation
  const create = useMutation({
    mutationFn: async () => {
      const route = await createRoute(projectId, { model, strategy });
      if (providerId) {
        await createRouteTarget(route.id, {
          provider_id: providerId,
          upstream_model: upstreamModel || undefined,
          weight: Number(weight) || 1,
        });
      }
      return route;
    },
    onSuccess: () => {
      onDone();
      onOpenChange(false);
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Add model</DialogTitle>
        <DialogDescription>
          Creates a DB-owned route. Add more targets after creation from Edit.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field label="Model name">
          <Input
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="gpt-4o"
          />
        </Field>
        <Field label="Strategy">
          <Select value={strategy} onChange={(e) => setStrategy(e.target.value)}>
            {STRATEGIES.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </Select>
        </Field>
        <Field
          label="Target provider"
          hint={providers.length ? undefined : "no providers configured for this org yet"}
        >
          <div className="flex items-center gap-1">
            <Select
              value={providerId}
              onChange={(e) => setProviderId(e.target.value)}
              className="flex-1"
            >
              <option value="">none (create route only)</option>
              {providers.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name} ({p.kind})
                </option>
              ))}
            </Select>
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={!orgId}
              title={orgId ? "Add a new provider" : "no org selected"}
              onClick={() => setNewProviderOpen(true)}
            >
              <Plus className="h-3.5 w-3.5" />
              New
            </Button>
          </div>
        </Field>
        {providerId && (
          <>
            <Field label="Upstream model (optional)">
              <Input
                value={upstreamModel}
                onChange={(e) => setUpstreamModel(e.target.value)}
                placeholder="defaults to the public model name"
              />
            </Field>
            <Field label="Weight">
              <Input
                type="number"
                min={1}
                value={weight}
                onChange={(e) => setWeight(e.target.value)}
              />
            </Field>
            {address && (
              <div className="flex items-center gap-2 rounded-md border border-border bg-muted/40 px-2 py-1.5">
                <span className="text-xs text-muted-foreground">Address</span>
                <span className="truncate font-mono text-xs">{address}</span>
                <CopyButton
                  value={address}
                  label="Copy provider-slug/model address"
                  className="ml-auto h-6 px-1.5"
                />
              </div>
            )}
          </>
        )}
        {orgId && (
          <AddProviderDialog
            open={newProviderOpen}
            onOpenChange={setNewProviderOpen}
            orgId={orgId}
            onDone={(created) => {
              onProvidersChanged();
              // select the freshly created provider so the binding continues
              setProviderId(created.id);
            }}
          />
        )}
        {create.isError && (
          <p className="text-xs text-destructive">{(create.error as Error).message}</p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button
          disabled={!model.trim() || create.isPending}
          onClick={() => create.mutate()}
        >
          Create
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

function EditModelDialog({
  route,
  onOpenChange,
  providers,
  onDone,
}: {
  route: RouteRow | null;
  onOpenChange: (open: boolean) => void;
  providers: ProviderRow[];
  onDone: () => void;
}) {
  const queryClient = useQueryClient();
  const open = !!route;

  const targets = useQuery({
    queryKey: ["route-targets", route?.id],
    queryFn: () => fetchRouteTargets(route!.id),
    enabled: open,
  });

  const [providerId, setProviderId] = React.useState("");
  const [upstreamModel, setUpstreamModel] = React.useState("");
  const [weight, setWeight] = React.useState("1");

  const [paramsText, setParamsText] = React.useState("{}");
  const [paramPolicyText, setParamPolicyText] = React.useState("{}");
  const [paramsError, setParamsError] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (open) {
      setProviderId(providers[0]?.id ?? "");
      setUpstreamModel("");
      setWeight("1");
      setParamsText(JSON.stringify(route?.params ?? {}, null, 2));
      setParamPolicyText(JSON.stringify(route?.param_policy ?? {}, null, 2));
      setParamsError(null);
    }
  }, [open, providers, route]);

  const invalidateTargets = () => {
    queryClient.invalidateQueries({ queryKey: ["route-targets", route?.id] });
    onDone();
  };

  const addTarget = useMutation({
    mutationFn: () =>
      createRouteTarget(route!.id, {
        provider_id: providerId,
        upstream_model: upstreamModel || undefined,
        weight: Number(weight) || 1,
      }),
    onSuccess: () => {
      setUpstreamModel("");
      setWeight("1");
      invalidateTargets();
    },
  });

  const removeTarget = useMutation({
    mutationFn: (id: string) => deleteRouteTarget(id),
    onSuccess: invalidateTargets,
  });

  const removeRoute = useMutation({
    mutationFn: () => deleteRoute(route!.id),
    onSuccess: () => {
      invalidateTargets();
      onOpenChange(false);
    },
  });

  const saveParams = useMutation({
    mutationFn: (input: {
      params: Record<string, unknown>;
      paramPolicy: Record<string, unknown>;
    }) => updateRouteParams(route!.id, input.params, input.paramPolicy),
    onSuccess: invalidateTargets,
  });

  const submitParams = () => {
    let params: Record<string, unknown>;
    let paramPolicy: Record<string, unknown>;
    try {
      params = JSON.parse(paramsText || "{}");
    } catch {
      setParamsError("params: invalid JSON");
      return;
    }
    try {
      paramPolicy = JSON.parse(paramPolicyText || "{}");
    } catch {
      setParamsError("param_policy: invalid JSON");
      return;
    }
    setParamsError(null);
    saveParams.mutate({ params, paramPolicy });
  };

  const providerName = (id: string) =>
    providers.find((p) => p.id === id)?.name ?? id;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Edit {route?.model}</DialogTitle>
        <DialogDescription>
          Manage targets for this route. Strategy changes aren't wired up yet — add
          a new route with the desired strategy if you need to change it.
        </DialogDescription>
      </DialogHeader>

      <div className="space-y-3">
        <div className="space-y-1.5">
          <p className="text-sm font-medium leading-none">Targets</p>
          {targets.isLoading && (
            <p className="text-xs text-muted-foreground">Loading…</p>
          )}
          {targets.data?.length === 0 && (
            <p className="text-xs text-muted-foreground">No targets yet.</p>
          )}
          <div className="space-y-1">
            {targets.data?.map((t) => (
              <div
                key={t.id}
                className="flex items-center justify-between gap-2 rounded-md border border-border bg-muted px-2 py-1.5 text-xs"
              >
                <span className="truncate font-mono">
                  {providerName(t.provider_id)}
                  {t.upstream_model ? ` → ${t.upstream_model}` : ""} (w{t.weight})
                </span>
                <button
                  type="button"
                  aria-label="Remove target"
                  onClick={() => removeTarget.mutate(t.id)}
                  className="shrink-0 text-muted-foreground hover:text-destructive"
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </button>
              </div>
            ))}
          </div>
        </div>

        <div className="space-y-2 rounded-md border border-dashed border-border p-3">
          <Field label="Provider">
            <Select value={providerId} onChange={(e) => setProviderId(e.target.value)}>
              {providers.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name} ({p.kind})
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Upstream model (optional)">
            <Input
              value={upstreamModel}
              onChange={(e) => setUpstreamModel(e.target.value)}
            />
          </Field>
          <Field label="Weight">
            <Input
              type="number"
              min={1}
              value={weight}
              onChange={(e) => setWeight(e.target.value)}
            />
          </Field>
          {addTarget.isError && (
            <p className="text-xs text-destructive">
              {(addTarget.error as Error).message}
            </p>
          )}
          <Button
            size="sm"
            variant="outline"
            disabled={!providerId || addTarget.isPending}
            onClick={() => addTarget.mutate()}
          >
            <Plus className="h-3.5 w-3.5" />
            Add target
          </Button>
        </div>
        <div className="space-y-2 rounded-md border border-dashed border-border p-3">
          <p className="text-sm font-medium leading-none">Params</p>
          <p className="text-xs text-muted-foreground">
            Admin default inference params and override policy, applied
            reload-free on save. Both fields must be JSON objects.
          </p>
          <Field label="Params">
            <Textarea
              className="font-mono text-xs"
              rows={4}
              value={paramsText}
              onChange={(e) => setParamsText(e.target.value)}
              placeholder='{"temperature": 0}'
            />
          </Field>
          <Field label="Param policy">
            <Textarea
              className="font-mono text-xs"
              rows={4}
              value={paramPolicyText}
              onChange={(e) => setParamPolicyText(e.target.value)}
              placeholder='{"mode": "merge", "allow": [], "deny": []}'
            />
          </Field>
          {paramsError && (
            <p className="text-xs text-destructive">{paramsError}</p>
          )}
          {saveParams.isError && (
            <p className="text-xs text-destructive">
              {(saveParams.error as Error).message}
            </p>
          )}
          <Button
            size="sm"
            variant="outline"
            disabled={saveParams.isPending}
            onClick={submitParams}
          >
            Save params
          </Button>
        </div>

        {removeRoute.isError && (
          <p className="text-xs text-destructive">
            {(removeRoute.error as Error).message}
          </p>
        )}
      </div>

      <DialogFooter>
        <Button
          variant="destructive"
          disabled={removeRoute.isPending}
          onClick={() => removeRoute.mutate()}
        >
          Delete route
        </Button>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Close
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
