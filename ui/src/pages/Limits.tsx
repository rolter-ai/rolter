import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
import * as React from "react";

import { PageBody } from "@/components/screen";
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
import {
  createBudget,
  createRateLimit,
  deleteBudget,
  deleteRateLimit,
  fetchBudgets,
  fetchRateLimits,
  SCOPE_TYPES,
  type BudgetRow,
  type RateLimitRow,
} from "@/lib/api";
import { useScope } from "@/lib/scope";

// budgets and rate limits share a scope (scope_type + scope_id), so this
// page combines both concerns behind one scope picker. defaults to the
// current project scope — pick another scope_type and paste an id to
// manage org/team/virtual-key scoped limits (no id lookup UI yet, see
// TODO.md follow-up)
export default function Limits() {
  const queryClient = useQueryClient();
  const scope = useScope();

  const [scopeType, setScopeType] = React.useState<string>("project");
  const [scopeId, setScopeId] = React.useState<string>("");

  React.useEffect(() => {
    if (scopeType === "project" && scope.projectId && !scopeId) {
      setScopeId(scope.projectId);
    }
  }, [scopeType, scope.projectId, scopeId]);

  const budgets = useQuery({
    queryKey: ["budgets", scopeType, scopeId],
    queryFn: () => fetchBudgets(scopeType, scopeId),
    enabled: !!scopeId,
  });

  const rateLimits = useQuery({
    queryKey: ["rate-limits", scopeType, scopeId],
    queryFn: () => fetchRateLimits(scopeType, scopeId),
    enabled: !!scopeId,
  });

  const invalidateBudgets = () =>
    queryClient.invalidateQueries({ queryKey: ["budgets", scopeType, scopeId] });
  const invalidateRateLimits = () =>
    queryClient.invalidateQueries({
      queryKey: ["rate-limits", scopeType, scopeId],
    });

  const removeBudget = useMutation({
    mutationFn: (id: string) => deleteBudget(id),
    onSuccess: invalidateBudgets,
  });

  const removeRateLimit = useMutation({
    mutationFn: (id: string) => deleteRateLimit(id),
    onSuccess: invalidateRateLimits,
  });

  const [addBudgetOpen, setAddBudgetOpen] = React.useState(false);
  const [addRateLimitOpen, setAddRateLimitOpen] = React.useState(false);

  const scopeBlocked = !scope.isLoading && !!scope.error;

  return (
    <PageBody className="gap-[22px]">

      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          Scope defaults are unavailable: {scope.error}. Pick a scope manually below.
        </p>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Scope</CardTitle>
          <CardDescription>
            Budgets and rate limits below apply to this scope.
          </CardDescription>
        </CardHeader>
        <CardContent className="grid gap-3 sm:grid-cols-2">
          <Field label="Scope type">
            <Select
              value={scopeType}
              onChange={(e) => {
                setScopeType(e.target.value);
                setScopeId(e.target.value === "project" ? scope.projectId ?? "" : "");
              }}
            >
              {SCOPE_TYPES.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Scope id" hint="uuid of the org/team/project/virtual key">
            <Input
              value={scopeId}
              onChange={(e) => setScopeId(e.target.value)}
              placeholder="00000000-0000-0000-0000-000000000000"
              className="font-mono text-xs"
            />
          </Field>
        </CardContent>
      </Card>

      <div className="space-y-3">
        <div className="flex items-center gap-3">
          <div className="flex flex-col gap-0.5">
            <h2 className="text-base font-medium">Budgets</h2>
            <span className="text-xs text-muted-foreground">
              Spend caps per org, team, project, or virtual key
            </span>
          </div>
          <Button
            size="sm"
            className="ml-auto"
            onClick={() => setAddBudgetOpen(true)}
            disabled={!scopeId}
          >
            <Plus className="h-4 w-4" />
            Add budget
          </Button>
        </div>
        {budgets.isLoading && <p className="text-sm text-muted-foreground">Loading…</p>}
        {budgets.error && (
          <p className="text-sm text-destructive">Failed to load budgets.</p>
        )}
        {!budgets.isLoading && scopeId && budgets.data?.length === 0 && (
          <p className="text-sm text-muted-foreground">No budgets for this scope.</p>
        )}
        <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(280px,1fr))]">
          {budgets.data?.map((budget) => (
            <BudgetCard
              key={budget.id}
              budget={budget}
              onDelete={() => removeBudget.mutate(budget.id)}
              deleting={removeBudget.isPending}
            />
          ))}
        </div>
      </div>

      <div className="space-y-3">
        <div className="flex items-center gap-3">
          <div className="flex flex-col gap-0.5">
            <h2 className="text-base font-medium">Rate limits</h2>
            <span className="text-xs text-muted-foreground">
              Requests- and tokens-per-minute caps per scope
            </span>
          </div>
          <Button
            size="sm"
            className="ml-auto"
            onClick={() => setAddRateLimitOpen(true)}
            disabled={!scopeId}
          >
            <Plus className="h-4 w-4" />
            Add rate limit
          </Button>
        </div>
        {rateLimits.isLoading && (
          <p className="text-sm text-muted-foreground">Loading…</p>
        )}
        {rateLimits.error && (
          <p className="text-sm text-destructive">Failed to load rate limits.</p>
        )}
        {!rateLimits.isLoading && scopeId && rateLimits.data?.length === 0 && (
          <p className="text-sm text-muted-foreground">
            No rate limits for this scope.
          </p>
        )}
        <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(280px,1fr))]">
          {rateLimits.data?.map((limit) => (
            <RateLimitCard
              key={limit.id}
              limit={limit}
              onDelete={() => removeRateLimit.mutate(limit.id)}
              deleting={removeRateLimit.isPending}
            />
          ))}
        </div>
      </div>

      <AddBudgetDialog
        open={addBudgetOpen}
        onOpenChange={setAddBudgetOpen}
        scopeType={scopeType}
        scopeId={scopeId}
        onDone={invalidateBudgets}
      />
      <AddRateLimitDialog
        open={addRateLimitOpen}
        onOpenChange={setAddRateLimitOpen}
        scopeType={scopeType}
        scopeId={scopeId}
        onDone={invalidateRateLimits}
      />
    </PageBody>
  );
}

function BudgetCard({
  budget,
  onDelete,
  deleting,
}: {
  budget: BudgetRow;
  onDelete: () => void;
  deleting: boolean;
}) {
  return (
    <div className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4">
      <div className="flex items-center gap-2.5">
        <span className="font-mono text-xl font-medium">${budget.limit_usd}</span>
        <Badge tone="outline">{budget.period}</Badge>
        <button
          type="button"
          title="Delete budget"
          disabled={deleting}
          onClick={onDelete}
          className="ml-auto flex rounded-[6px] border border-[color:var(--border-subtle)] px-1.5 py-1 text-[color:var(--status-danger)] transition-colors hover:bg-[color:var(--red-tint)]"
        >
          <Trash2 className="h-3.5 w-3.5" />
        </button>
      </div>
      <div className="flex items-center gap-1.5">
        <Badge tone="neutral">{budget.scope_type}</Badge>
        <span className="truncate font-mono text-xs text-[color:var(--text-secondary)]">
          {budget.scope_id}
        </span>
      </div>
    </div>
  );
}

function RateLimitCard({
  limit,
  onDelete,
  deleting,
}: {
  limit: RateLimitRow;
  onDelete: () => void;
  deleting: boolean;
}) {
  return (
    <div className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4">
      <div className="flex flex-wrap items-center gap-2">
        {limit.rpm != null && <Badge tone="outline">{limit.rpm} rpm</Badge>}
        {limit.tpm != null && <Badge tone="outline">{limit.tpm} tpm</Badge>}
        {limit.rpm == null && limit.tpm == null && <Badge tone="neutral">no caps</Badge>}
        <button
          type="button"
          title="Delete rate limit"
          disabled={deleting}
          onClick={onDelete}
          className="ml-auto flex rounded-[6px] border border-[color:var(--border-subtle)] px-1.5 py-1 text-[color:var(--status-danger)] transition-colors hover:bg-[color:var(--red-tint)]"
        >
          <Trash2 className="h-3.5 w-3.5" />
        </button>
      </div>
      <div className="flex items-center gap-1.5">
        <Badge tone="neutral">{limit.scope_type}</Badge>
        <span className="truncate font-mono text-xs text-[color:var(--text-secondary)]">
          {limit.scope_id}
        </span>
      </div>
    </div>
  );
}

function AddBudgetDialog({
  open,
  onOpenChange,
  scopeType,
  scopeId,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  scopeType: string;
  scopeId: string;
  onDone: () => void;
}) {
  const [limitUsd, setLimitUsd] = React.useState("100");
  const [period, setPeriod] = React.useState("30d");

  React.useEffect(() => {
    if (open) {
      setLimitUsd("100");
      setPeriod("30d");
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () =>
      createBudget({
        scope_type: scopeType,
        scope_id: scopeId,
        limit_usd: limitUsd,
        period,
      }),
    onSuccess: () => {
      onDone();
      onOpenChange(false);
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Add budget</DialogTitle>
        <DialogDescription>
          Spend cap for <span className="font-mono">{scopeType}:{scopeId}</span>.
          There's no update endpoint — delete and recreate to change it.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field label="Limit (USD)">
          <Input
            type="number"
            min={0}
            step="0.01"
            value={limitUsd}
            onChange={(e) => setLimitUsd(e.target.value)}
          />
        </Field>
        <Field label="Period" hint="e.g. 30d, 7d, 1d">
          <Input value={period} onChange={(e) => setPeriod(e.target.value)} />
        </Field>
        {create.isError && (
          <p className="text-xs text-destructive">{(create.error as Error).message}</p>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button
          disabled={!limitUsd.trim() || !period.trim() || create.isPending}
          onClick={() => create.mutate()}
        >
          Create
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

function AddRateLimitDialog({
  open,
  onOpenChange,
  scopeType,
  scopeId,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  scopeType: string;
  scopeId: string;
  onDone: () => void;
}) {
  const [rpm, setRpm] = React.useState("");
  const [tpm, setTpm] = React.useState("");

  React.useEffect(() => {
    if (open) {
      setRpm("");
      setTpm("");
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () =>
      createRateLimit({
        scope_type: scopeType,
        scope_id: scopeId,
        rpm: rpm.trim() ? Number(rpm) : undefined,
        tpm: tpm.trim() ? Number(tpm) : undefined,
      }),
    onSuccess: () => {
      onDone();
      onOpenChange(false);
    },
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogHeader>
        <DialogTitle>Add rate limit</DialogTitle>
        <DialogDescription>
          Throughput caps for <span className="font-mono">{scopeType}:{scopeId}</span>.
          Leave a field blank to leave it uncapped.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3">
        <Field label="Requests per minute (optional)">
          <Input
            type="number"
            min={0}
            value={rpm}
            onChange={(e) => setRpm(e.target.value)}
            placeholder="unlimited"
          />
        </Field>
        <Field label="Tokens per minute (optional)">
          <Input
            type="number"
            min={0}
            value={tpm}
            onChange={(e) => setTpm(e.target.value)}
            placeholder="unlimited"
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
        <Button
          disabled={(!rpm.trim() && !tpm.trim()) || create.isPending}
          onClick={() => create.mutate()}
        >
          Create
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
