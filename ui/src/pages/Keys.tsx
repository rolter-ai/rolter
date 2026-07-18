import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy, Plus, Trash2 } from "lucide-react";
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
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tag } from "@/components/ui/tag";
import {
  createVirtualKey,
  deleteVirtualKey,
  fetchVirtualKeys,
  setVirtualKeyCache,
  setVirtualKeyDisabled,
  type CreatedVirtualKey,
  type VirtualKeyRow,
} from "@/lib/api";
import { useScope } from "@/lib/scope";

const KEYS_QUERY_KEY = ["virtual-keys"];

export default function Keys() {
  const queryClient = useQueryClient();
  const scope = useScope();

  const keys = useQuery({
    queryKey: [...KEYS_QUERY_KEY, scope.projectId],
    queryFn: () => fetchVirtualKeys(scope.projectId as string),
    enabled: !!scope.projectId,
  });

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: [...KEYS_QUERY_KEY, scope.projectId] });

  const toggleDisabled = useMutation({
    mutationFn: ({ id, disabled }: { id: string; disabled: boolean }) =>
      setVirtualKeyDisabled(id, disabled),
    onSuccess: invalidate,
  });

  const setCache = useMutation({
    mutationFn: ({ id, cache }: { id: string; cache: boolean | null }) =>
      setVirtualKeyCache(id, cache),
    onSuccess: invalidate,
  });

  const removeKey = useMutation({
    mutationFn: (id: string) => deleteVirtualKey(id),
    onSuccess: invalidate,
  });

  const [addOpen, setAddOpen] = React.useState(false);
  const [deleteTarget, setDeleteTarget] = React.useState<VirtualKeyRow | null>(null);
  const [created, setCreated] = React.useState<CreatedVirtualKey | null>(null);

  const scopeBlocked = !scope.isLoading && !!scope.error;

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-4">
        <div>
          <h1 className="text-2xl font-semibold">Virtual keys</h1>
          <p className="text-sm text-muted-foreground">
            API keys clients use to call rolter, scoped to a project.
          </p>
        </div>
        <Button
          size="sm"
          onClick={() => setAddOpen(true)}
          disabled={scopeBlocked || !scope.projectId}
        >
          <Plus className="h-4 w-4" />
          Create key
        </Button>
      </div>

      {keys.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
      {keys.error && (
        <p className="text-sm text-destructive">Failed to load virtual keys.</p>
      )}
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          Add/edit/delete is unavailable: {scope.error}. Read-only view still works.
        </p>
      )}

      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-3">
        {keys.data?.map((key) => (
          <Card key={key.id}>
            <CardHeader>
              <CardTitle className="flex items-center justify-between gap-2">
                <span className="truncate">{key.name ?? "unnamed key"}</span>
                <Switch
                  checked={!key.disabled}
                  disabled={toggleDisabled.isPending}
                  onCheckedChange={(enabled) =>
                    toggleDisabled.mutate({ id: key.id, disabled: !enabled })
                  }
                />
              </CardTitle>
              <CardDescription className="font-mono">
                {key.key_prefix}…
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-2">
              <div className="flex flex-wrap gap-1.5">
                {key.models.length ? (
                  key.models.map((model) => <Tag key={model}>{model}</Tag>)
                ) : (
                  <Badge tone="neutral">all models</Badge>
                )}
              </div>
              <Field
                label="Response cache"
                hint="The global cache switch must also be enabled."
              >
                <Select
                  aria-label={`Response cache policy for ${key.name ?? key.key_prefix}`}
                  value={cacheMode(key.cache_enabled)}
                  disabled={setCache.isPending}
                  onChange={(event) =>
                    setCache.mutate({ id: key.id, cache: parseCacheMode(event.target.value) })
                  }
                >
                  <option value="inherit">Inherit route setting</option>
                  <option value="off">Off</option>
                  <option value="on">On</option>
                </Select>
              </Field>
              <div className="flex items-center justify-between gap-2 text-sm text-muted-foreground">
                <span>
                  {key.expires_at
                    ? `expires ${new Date(key.expires_at).toLocaleDateString()}`
                    : "no expiry"}
                </span>
                <Button
                  size="sm"
                  variant="destructive"
                  onClick={() => setDeleteTarget(key)}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            </CardContent>
          </Card>
        ))}
      </div>

      {scope.projectId && (
        <AddKeyDialog
          open={addOpen}
          onOpenChange={setAddOpen}
          projectId={scope.projectId}
          onCreated={(key) => {
            invalidate();
            setCreated(key);
          }}
        />
      )}

      <Dialog open={!!deleteTarget} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <DialogHeader>
          <DialogTitle>Delete virtual key</DialogTitle>
          <DialogDescription>
            <span className="font-mono">{deleteTarget?.key_prefix}…</span> will stop
            authenticating immediately. This cannot be undone.
          </DialogDescription>
        </DialogHeader>
        {removeKey.isError && (
          <p className="text-xs text-destructive">
            {(removeKey.error as Error).message}
          </p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => setDeleteTarget(null)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={removeKey.isPending}
            onClick={() => {
              if (!deleteTarget) return;
              removeKey.mutate(deleteTarget.id, {
                onSuccess: () => setDeleteTarget(null),
              });
            }}
          >
            Delete
          </Button>
        </DialogFooter>
      </Dialog>

      <CreatedKeyDialog created={created} onOpenChange={(open) => !open && setCreated(null)} />
    </div>
  );
}

function AddKeyDialog({
  open,
  onOpenChange,
  projectId,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  projectId: string;
  onCreated: (key: CreatedVirtualKey) => void;
}) {
  const [name, setName] = React.useState("");
  const [modelsText, setModelsText] = React.useState("");
  const [cache, setCache] = React.useState<"inherit" | "off" | "on">("inherit");

  React.useEffect(() => {
    if (open) {
      setName("");
      setModelsText("");
      setCache("inherit");
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () =>
      createVirtualKey(projectId, {
        name: name || undefined,
        models: modelsText
          .split(",")
          .map((m) => m.trim())
          .filter(Boolean),
        cache: parseCacheMode(cache),
      }),
    onSuccess: (key) => {
      onOpenChange(false);
      onCreated(key);
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Create virtual key</DialogTitle>
        <DialogDescription>
          The plaintext key is shown once, right after creation — copy it then.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field label="Name (optional)">
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="backend service"
          />
        </Field>
        <Field
          label="Response cache"
          hint="Inherit uses the route setting; the deployment-wide cache switch still applies."
        >
          <Select value={cache} onChange={(event) => setCache(event.target.value as typeof cache)}>
            <option value="inherit">Inherit route setting</option>
            <option value="off">Off</option>
            <option value="on">On</option>
          </Select>
        </Field>
        <Field
          label="Model allow-list (optional)"
          hint="comma-separated public model names; empty allows all models"
        >
          <Input
            value={modelsText}
            onChange={(e) => setModelsText(e.target.value)}
            placeholder="gpt-4o, claude-sonnet"
          />
        </Field>
        {create.isError && (
          <p className="text-xs text-destructive">{(create.error as Error).message}</p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button disabled={create.isPending} onClick={() => create.mutate()}>
          Create
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

function cacheMode(cache: boolean | null | undefined): "inherit" | "off" | "on" {
  if (cache === true) return "on";
  if (cache === false) return "off";
  return "inherit";
}

function parseCacheMode(value: string): boolean | null {
  if (value === "on") return true;
  if (value === "off") return false;
  return null;
}

// shows the plaintext secret exactly once, right after creation; state is
// local to this dialog and is discarded on close, never re-fetchable
function CreatedKeyDialog({
  created,
  onOpenChange,
}: {
  created: CreatedVirtualKey | null;
  onOpenChange: (open: boolean) => void;
}) {
  const [copied, setCopied] = React.useState(false);

  React.useEffect(() => {
    if (created) setCopied(false);
  }, [created]);

  const copy = async () => {
    if (!created) return;
    try {
      await navigator.clipboard.writeText(created.key);
      setCopied(true);
    } catch {
      // clipboard unavailable — user can still select/copy the text manually
    }
  };

  return (
    <Dialog open={!!created} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Key created</DialogTitle>
        <DialogDescription>
          This is the only time the plaintext key is shown. Copy it now — it can't be
          retrieved again.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-2 rounded-md border border-dashed border-border bg-muted p-3">
        <div className="flex items-center justify-between gap-2">
          <code className="break-all text-sm">{created?.key}</code>
          <Button size="sm" variant="outline" onClick={copy}>
            {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
          </Button>
        </div>
      </div>
      <DialogFooter>
        <Button onClick={() => onOpenChange(false)}>Done</Button>
      </DialogFooter>
    </Dialog>
  );
}
