import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Gauge, Plus, Trash2, Loader2, Wallet } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { EditorSheet } from "@/components/EditorSheet";
import { PageBody } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import {
  createBudget,
  createRateLimit,
  deleteBudget,
  deleteRateLimit,
  fetchBudgets,
  fetchBusinessUnits,
  fetchCustomers,
  fetchRateLimits,
  fetchVirtualKeys,
  SCOPE_TYPES,
  UNPRICED_POLICIES,
  type BudgetRow,
  type UnpricedPolicy,
  type RateLimitRow,
} from "@/lib/api";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// budgets and rate limits share a scope (scope_type + scope_id), so this
// page combines both concerns behind one scope picker. defaults to the
// current project scope — pick another scope_type and paste an id to
// manage org/team/virtual-key scoped limits.
export default function Limits() {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const scope = useScope();
  // the scope hook names a catalog key rather than carrying english copy
  const scopeMessage = scope.errorKey ? t(scope.errorKey) : undefined;

  const [scopeType, setScopeType] = React.useState<string>("project");
  const [scopeId, setScopeId] = React.useState<string>("");

  React.useEffect(() => {
    if (scopeType === "project" && scope.projectId && !scopeId) {
      setScopeId(scope.projectId);
    }
  }, [scopeType, scope.projectId, scopeId]);


  const virtualKeys = useQuery({
    queryKey: ["virtual-keys", scope.projectId],
    queryFn: () => fetchVirtualKeys(scope.projectId as string),
    enabled: scopeType === "virtual_key" && !!scope.projectId,
  });

  // the two governance dimensions a key's spend rolls up to (#539). they are
  // org-scoped rather than part of the org/team/project chain useScope walks,
  // so each is its own query, fetched only when that scope is picked
  const businessUnits = useQuery({
    queryKey: ["business-units", scope.orgId],
    queryFn: () => fetchBusinessUnits(scope.orgId as string),
    enabled: scopeType === "business_unit" && !!scope.orgId,
  });

  const customers = useQuery({
    queryKey: ["customers", scope.orgId],
    queryFn: () => fetchCustomers(scope.orgId as string),
    enabled: scopeType === "customer" && !!scope.orgId,
  });



  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;


  // `virtualKeys` is the query the user is actually waiting on for this screen


  useScreenReady(!virtualKeys.isLoading);


  useErrorState(!!virtualKeys.error, "limits");

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
    onSuccess: () => {
      invalidateBudgets();
      toast.push({ tone: "success", title: t("pages.limits.budgetDeleted") });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.deleteFailed", { what: t("pages.limits.budgetNoun") }),
        detail: errorDetail(error),
      });
    },
  });

  const removeRateLimit = useMutation({
    mutationFn: (id: string) => deleteRateLimit(id),
    onSuccess: () => {
      invalidateRateLimits();
      toast.push({ tone: "success", title: t("pages.limits.rateLimitDeleted") });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.deleteFailed", { what: t("pages.limits.rateLimitNoun") }),
        detail: errorDetail(error),
      });
    },
  });

  const [addBudgetOpen, setAddBudgetOpen] = React.useState(false);
  const [addRateLimitOpen, setAddRateLimitOpen] = React.useState(false);

  const scopeBlocked = !scope.isLoading && !!scope.errorKey;

  return (
    <PageBody className="gap-[22px]">

      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          Scope defaults are unavailable: {scopeMessage}. Pick a scope manually below.
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
              {SCOPE_TYPES.map((type) => (
                <option key={type} value={type}>
                  {t(`pages.limits.scopeTypes.${type}`)}
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Scope" hint={t("pages.limits.scopeHint", { type: t(`pages.limits.scopeTypes.${scopeType}`) })}>
            {scopeType === "org" && scope.orgs.length > 0 ? (
              <Select value={scopeId} onChange={(e) => setScopeId(e.target.value)}>
                <option value="">Select an org</option>
                {scope.orgs.map((o) => (
                  <option key={o.id} value={o.id}>
                    {o.name}
                  </option>
                ))}
              </Select>
            ) : scopeType === "team" && scope.teams.length > 0 ? (
              <Select value={scopeId} onChange={(e) => setScopeId(e.target.value)}>
                <option value="">Select a team</option>
                {scope.teams.map((t) => (
                  <option key={t.id} value={t.id}>
                    {t.name}
                  </option>
                ))}
              </Select>
            ) : scopeType === "project" && scope.projects.length > 0 ? (
              <Select value={scopeId} onChange={(e) => setScopeId(e.target.value)}>
                <option value="">Select a project</option>
                {scope.projects.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </Select>
            ) : scopeType === "virtual_key" && virtualKeys.data && virtualKeys.data.length > 0 ? (
              <Select value={scopeId} onChange={(e) => setScopeId(e.target.value)}>
                <option value="">Select a virtual key</option>
                {virtualKeys.data.map((k) => (
                  <option key={k.id} value={k.id}>
                    {k.name || k.key_prefix}
                  </option>
                ))}
              </Select>
            ) : scopeType === "business_unit" &&
              businessUnits.data &&
              businessUnits.data.length > 0 ? (
              <Select value={scopeId} onChange={(e) => setScopeId(e.target.value)}>
                <option value="">{t("pages.limits.selectBusinessUnit")}</option>
                {businessUnits.data.map((u) => (
                  <option key={u.id} value={u.id}>
                    {u.name}
                  </option>
                ))}
              </Select>
            ) : scopeType === "customer" && customers.data && customers.data.length > 0 ? (
              <Select value={scopeId} onChange={(e) => setScopeId(e.target.value)}>
                <option value="">{t("pages.limits.selectCustomer")}</option>
                {customers.data.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
              </Select>
            ) : (
              <Input
                value={scopeId}
                onChange={(e) => setScopeId(e.target.value)}
                placeholder="00000000-0000-0000-0000-000000000000"
                className="font-mono text-xs"
              />
            )}
          </Field>
        </CardContent>
      </Card>

      <div className="space-y-3">
        <div className="flex items-center gap-3">
          <div className="flex flex-col gap-0.5">
            <h2 className="text-base font-medium">Budgets</h2>
            <span className="text-xs text-muted-foreground">
              {t("pages.limits.budgetsHint")}
            </span>
          </div>
          <GatedButton
            gate="budget:create"
            size="sm"
            className="ml-auto"
            onClick={() => setAddBudgetOpen(true)}
            disabled={!scopeId}
          >
            <Plus className="h-4 w-4" />
            Add budget
          </GatedButton>
        </div>
        {budgets.isLoading && <CardGridSkeleton cards={3} height={152} min={280} />}
        {budgets.error && (
          <LoadError
            error={budgets.error}
            resource={t("errors.resources.budgets")}
            onRetry={() => budgets.refetch()}
          />
        )}
        {!budgets.isLoading && scopeId && budgets.data?.length === 0 && (
          <EmptyState
            uxTarget="budgets"
            icon={<Wallet />}
            title={t("pages.limits.budgetsEmptyTitle")}
            description={t("pages.limits.budgetsEmptyBody")}
            actions={
              <GatedButton gate="budget:create" disabled={!scopeId} onClick={() => setAddBudgetOpen(true)}>
                {t("pages.limits.budgetsEmptyAction")}
              </GatedButton>
            }
          />
        )}
        <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(280px,100%),1fr))]">
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
              {t("pages.limits.rateLimitsHint")}
            </span>
          </div>
          <GatedButton
            gate="rate_limit:create"
            size="sm"
            className="ml-auto"
            onClick={() => setAddRateLimitOpen(true)}
            disabled={!scopeId}
          >
            <Plus className="h-4 w-4" />
            Add rate limit
          </GatedButton>
        </div>
        {rateLimits.isLoading && <CardGridSkeleton cards={3} height={152} min={280} />}
        {rateLimits.error && (
          <LoadError
            error={rateLimits.error}
            resource={t("errors.resources.rateLimits")}
            onRetry={() => rateLimits.refetch()}
          />
        )}
        {!rateLimits.isLoading && scopeId && rateLimits.data?.length === 0 && (
          <EmptyState
            uxTarget="rate-limits"
            icon={<Gauge />}
            title={t("pages.limits.rateLimitsEmptyTitle")}
            description={t("pages.limits.rateLimitsEmptyBody")}
            actions={
              <GatedButton gate="rate_limit:create" disabled={!scopeId} onClick={() => setAddRateLimitOpen(true)}>
                {t("pages.limits.rateLimitsEmptyAction")}
              </GatedButton>
            }
          />
        )}
        <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(280px,100%),1fr))]">
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
  // budgets are denominated in the deployment's settlement currency, not in
  // dollars — the `_usd` in the column name is historic (#1182)
  const { t } = useTranslation();
  const fmt = useFormat();
  const currency = useCurrencyCode();
  return (
    <div className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-default)] bg-card p-4">
      <div className="flex flex-wrap items-center gap-2.5">
        <span className="font-mono text-xl font-medium">
          {fmt.currency(Number(budget.limit_usd), currency)}
        </span>
        <Badge tone="outline">{budget.period}</Badge>
        {/* an override is worth showing on the card because it changes what
            the gateway will serve, not just what it counts (#996). a budget
            without one inherits the deployment setting and says nothing */}
        {budget.unpriced_policy && (
          <Badge tone={budget.unpriced_policy === "block" ? "danger" : "warning"}>
            {t("pages.limits.unpricedBadge", {
              policy: t(`pages.limits.unpriced.${budget.unpriced_policy}`),
            })}
          </Badge>
        )}
        <button
          type="button"
          title="Delete budget"
          aria-label="Delete budget"
          disabled={deleting}
          onClick={onDelete}
          className="ml-auto flex rounded-[6px] border border-[color:var(--border-subtle)] px-1.5 py-1 text-[color:var(--status-danger-text)] transition-colors hover:bg-[color:var(--red-tint)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50 disabled:pointer-events-none"
        >
          {deleting ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Trash2 className="h-3.5 w-3.5" />}
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
          aria-label="Delete rate limit"
          disabled={deleting}
          onClick={onDelete}
          className="ml-auto flex rounded-[6px] border border-[color:var(--border-subtle)] px-1.5 py-1 text-[color:var(--status-danger-text)] transition-colors hover:bg-[color:var(--red-tint)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50 disabled:pointer-events-none"
        >
          {deleting ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Trash2 className="h-3.5 w-3.5" />}
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
  const { t } = useTranslation();
  const toast = useToast();
  const [limitUsd, setLimitUsd] = React.useState("100");
  const [period, setPeriod] = React.useState("30d");
  // "" is the inherit case, which is what the API means by a null override
  const [unpriced, setUnpriced] = React.useState<UnpricedPolicy | "">("");

  React.useEffect(() => {
    if (open) {
      setLimitUsd("100");
      setPeriod("30d");
      setUnpriced("");
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () =>
      createBudget({
        scope_type: scopeType,
        scope_id: scopeId,
        limit_usd: limitUsd,
        period,
        unpriced_policy: unpriced === "" ? null : unpriced,
      }),
    onSuccess: () => {
      // the dialog closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({ tone: "success", title: t("pages.limits.budgetCreated") });
      onDone();
      onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("pages.limits.budgetNoun") }),
        detail: errorDetail(error),
      });
    },
  });

  return (
    <EditorSheet
      open={open}
      onOpenChange={onOpenChange}
      title="Add budget"
      subtitle={`Spend cap for ${scopeType}:${scopeId} — delete and recreate to change it`}
      dirty={limitUsd !== "100" || period !== "30d" || unpriced !== ""}
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      saveLabel="Create"
      canSave={Boolean(limitUsd.trim() && period.trim())}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
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
        <Field
          label={t("pages.limits.unpricedLabel")}
          hint={t("pages.limits.unpricedHint")}
          htmlFor="budget-unpriced-policy"
        >
          <Select
            id="budget-unpriced-policy"
            value={unpriced}
            onChange={(e) => setUnpriced(e.target.value as UnpricedPolicy | "")}
          >
            <option value="">{t("pages.limits.unpriced.inherit")}</option>
            {UNPRICED_POLICIES.map((policy) => (
              <option key={policy} value={policy}>
                {t(`pages.limits.unpriced.${policy}`)}
              </option>
            ))}
          </Select>
        </Field>
      </div>
    </EditorSheet>
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
  const { t } = useTranslation();
  const toast = useToast();
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
      // the dialog closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({ tone: "success", title: t("pages.limits.rateLimitCreated") });
      onDone();
      onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("pages.limits.rateLimitNoun") }),
        detail: errorDetail(error),
      });
    },
  });

  return (
    <EditorSheet
      open={open}
      onOpenChange={onOpenChange}
      title="Add rate limit"
      subtitle={`Throughput caps for ${scopeType}:${scopeId} — blank leaves a field uncapped`}
      dirty={Boolean(rpm || tpm)}
      errorMessage={create.isError ? (create.error as Error).message : undefined}
      saveLabel="Create"
      canSave={Boolean(rpm.trim() || tpm.trim())}
      saving={create.isPending}
      onSave={() => create.mutate()}
    >
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
      </div>
    </EditorSheet>
  );
}
