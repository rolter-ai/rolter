import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Gauge, Pencil, Plus, Wallet } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { EditorSheet } from "@/components/EditorSheet";
import { PageBody, RowIconButton } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardStack,
  CardTitle,
} from "@/components/ui/card";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";

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
  updateBudget,
  updateRateLimit,
  type BudgetRow,
  type UnpricedPolicy,
  type RateLimitRow,
  type UpdateBudgetInput,
  type UpdateRateLimitInput,
} from "@/lib/api";
import { PERIOD_KINDS, periodKind } from "@/lib/budget-period";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { RowCapabilityScope, type RowScope } from "@/lib/can";
import { capGateScope } from "@/lib/limit-scope";
import { useOrgScope } from "@/components/OrgScopePicker";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// the picker's placeholder per scope type, as catalog keys
const SCOPE_PLACEHOLDERS: Record<string, string> = {
  org: "pages.limits.selectOrg",
  team: "pages.limits.selectTeam",
  project: "pages.limits.selectProject",
  virtual_key: "pages.limits.selectVirtualKey",
  business_unit: "pages.limits.selectBusinessUnit",
  customer: "pages.limits.selectCustomer",
};

// budgets and rate limits share a scope (scope_type + scope_id), so this
// page combines both concerns behind one scope picker. defaults to the
// current project scope — pick another scope_type and paste an id to
// manage org/team/virtual-key scoped limits.
export default function Limits() {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const scope = useScope();
  const orgScope = useOrgScope(scope.orgId);
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

  // the row a delete confirmation is open for. a cap deleted on a misclick
  // leaves its scope uncapped until someone notices, so neither delete leaves
  // before the dialog is answered (#1904)
  const [budgetToDelete, setBudgetToDelete] = React.useState<BudgetRow | null>(null);
  const [rateLimitToDelete, setRateLimitToDelete] = React.useState<RateLimitRow | null>(null);

  const [addBudgetOpen, setAddBudgetOpen] = React.useState(false);
  const [addRateLimitOpen, setAddRateLimitOpen] = React.useState(false);
  // the row being edited outlives its sheet closing, so the closing sheet keeps
  // its own name and title rather than reading as the create form (#1285)
  const [editingBudget, setEditingBudget] = React.useState<BudgetRow | null>(null);
  const [editBudgetOpen, setEditBudgetOpen] = React.useState(false);
  const [editingRateLimit, setEditingRateLimit] = React.useState<RateLimitRow | null>(null);
  const [editRateLimitOpen, setEditRateLimitOpen] = React.useState(false);

  const scopeBlocked = !scope.isLoading && !!scope.errorKey;

  // what the scope picker offers for the chosen type. the same list names the
  // scope on the cards' controls and in the delete confirmations, so a row is
  // never introduced by its uuid when the picker already knows its name
  const scopeOptions = (() => {
    const named = (rows: { id: string; name: string }[]) =>
      rows.map((row) => ({ value: row.id, label: row.name }));
    switch (scopeType) {
      case "org":
        return named(scope.orgs);
      case "team":
        return named(scope.teams);
      case "project":
        return named(scope.projects);
      case "virtual_key":
        return (virtualKeys.data ?? []).map((k) => ({
          value: k.id,
          label: k.name || k.key_prefix,
        }));
      case "business_unit":
        return named(businessUnits.data ?? []);
      case "customer":
        return named(customers.data ?? []);
      default:
        return [];
    }
  })();

  // the scope field is a name picker whenever the scope type has rows to offer
  // and a bare uuid box otherwise; the hint has to say which one it is, since
  // "the project this cap applies to" reads as nonsense over an empty uuid
  // field (#1202)
  const hasPicker = scopeOptions.length > 0;
  // a scope typed in as a uuid has no name to show, and is shown as that uuid
  const scopeName = scopeOptions.find((option) => option.value === scopeId)?.label ?? scopeId;
  // the delete confirmations open on it, so the type comes with the name
  const scopeLabel = t("pages.limits.scopeNamed", {
    type: t(`pages.limits.scopeTypes.${scopeType}`),
    name: scopeName,
  });
  const budgetNames = useBudgetNames(scopeName);
  const rateLimitCaps = useRateLimitCaps();

  return (
    <PageBody className="gap-[22px]">
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          {t("pages.limits.scopeBlocked", { detail: scopeMessage })}
        </p>
      )}

      <Card>
        <CardHeader>
          <CardTitle>{t("pages.limits.scopeTitle")}</CardTitle>
          <CardDescription>{t("pages.limits.scopeBody")}</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-3 sm:grid-cols-2">
          <Field label={t("pages.limits.scopeTypeLabel")}>
            <Combobox
              value={scopeType}
              onChange={(picked) => {
                setScopeType(picked);
                setScopeId(picked === "project" ? (scope.projectId ?? "") : "");
              }}
              options={SCOPE_TYPES.map((type) => ({
                value: type,
                label: t(`pages.limits.scopeTypes.${type}`),
              }))}
            />
          </Field>
          <Field
            label={t("pages.limits.scopeLabel")}
            hint={t(hasPicker ? "pages.limits.scopeHint" : "pages.limits.scopeIdHint", {
              type: t(`pages.limits.scopeTypes.${scopeType}`),
            })}
          >
            {hasPicker ? (
              <Combobox
                value={scopeId}
                onChange={setScopeId}
                placeholder={t(SCOPE_PLACEHOLDERS[scopeType])}
                options={scopeOptions}
              />
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
            <h2 className="text-base font-medium">{t("pages.limits.budgetsTitle")}</h2>
            <span className="text-xs text-muted-foreground">{t("pages.limits.budgetsHint")}</span>
          </div>
          <GatedButton
            gate="budget:create"
            control="budget-new"
            size="sm"
            className="ml-auto"
            onClick={() => setAddBudgetOpen(true)}
            disabled={!scopeId}
          >
            <Plus className="h-4 w-4" />
            {t("pages.limits.budgetsAdd")}
          </GatedButton>
        </div>
        {budgets.isLoading && <CardGridSkeleton cards={3} height={94} min={280} />}
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
              <GatedButton
                gate="budget:create"
                control="budget-new-empty"
                disabled={!scopeId}
                onClick={() => setAddBudgetOpen(true)}
              >
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
              scope={scopeName}
              gateAt={capGateScope(budget, {
                byTeam: orgScope.byTeam,
                keyProjectId: scope.projectId,
              })}
              onEdit={() => {
                setEditingBudget(budget);
                setEditBudgetOpen(true);
              }}
              onDelete={() => setBudgetToDelete(budget)}
              // the mutation is shared by every card, so only the one it was
              // given spins
              deleting={removeBudget.isPending && budgetToDelete?.id === budget.id}
            />
          ))}
        </div>
      </div>

      <div className="space-y-3">
        <div className="flex items-center gap-3">
          <div className="flex flex-col gap-0.5">
            <h2 className="text-base font-medium">{t("pages.limits.rateLimitsTitle")}</h2>
            <span className="text-xs text-muted-foreground">
              {t("pages.limits.rateLimitsHint")}
            </span>
          </div>
          <GatedButton
            gate="rate_limit:create"
            control="rate-limit-new"
            size="sm"
            className="ml-auto"
            onClick={() => setAddRateLimitOpen(true)}
            disabled={!scopeId}
          >
            <Plus className="h-4 w-4" />
            {t("pages.limits.rateLimitsAdd")}
          </GatedButton>
        </div>
        {rateLimits.isLoading && <CardGridSkeleton cards={3} height={94} min={280} />}
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
              <GatedButton
                gate="rate_limit:create"
                control="rate-limit-new-empty"
                disabled={!scopeId}
                onClick={() => setAddRateLimitOpen(true)}
              >
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
              scope={scopeName}
              gateAt={capGateScope(limit, {
                byTeam: orgScope.byTeam,
                keyProjectId: scope.projectId,
              })}
              onEdit={() => {
                setEditingRateLimit(limit);
                setEditRateLimitOpen(true);
              }}
              onDelete={() => setRateLimitToDelete(limit)}
              deleting={removeRateLimit.isPending && rateLimitToDelete?.id === limit.id}
            />
          ))}
        </div>
      </div>

      <BudgetSheet
        open={addBudgetOpen}
        onOpenChange={setAddBudgetOpen}
        scopeType={scopeType}
        scopeId={scopeId}
        scopeName={scopeName}
        onDone={invalidateBudgets}
      />
      {editingBudget && (
        <BudgetSheet
          open={editBudgetOpen}
          onOpenChange={setEditBudgetOpen}
          scopeType={editingBudget.scope_type}
          scopeId={editingBudget.scope_id}
          scopeName={scopeName}
          budget={editingBudget}
          onDone={invalidateBudgets}
        />
      )}
      <RateLimitSheet
        open={addRateLimitOpen}
        onOpenChange={setAddRateLimitOpen}
        scopeType={scopeType}
        scopeId={scopeId}
        scopeName={scopeName}
        onDone={invalidateRateLimits}
      />
      {editingRateLimit && (
        <RateLimitSheet
          open={editRateLimitOpen}
          onOpenChange={setEditRateLimitOpen}
          scopeType={editingRateLimit.scope_type}
          scopeId={editingRateLimit.scope_id}
          scopeName={scopeName}
          limit={editingRateLimit}
          onDone={invalidateRateLimits}
        />
      )}

      <ConfirmDialog
        name="budget-delete"
        open={!!budgetToDelete}
        onOpenChange={(open) => {
          if (open) return;
          setBudgetToDelete(null);
          // a refusal for this budget must not greet the next one opened
          removeBudget.reset();
        }}
        title={
          budgetToDelete ? t("pages.limits.confirm.budgetTitle", budgetNames(budgetToDelete)) : ""
        }
        description={
          budgetToDelete ? t("pages.limits.confirm.budgetBody", { scope: scopeLabel }) : ""
        }
        confirmLabel={t("pages.limits.confirm.budgetConfirm")}
        pending={removeBudget.isPending}
        error={removeBudget.error}
        onConfirm={() =>
          budgetToDelete &&
          removeBudget.mutate(budgetToDelete.id, { onSuccess: () => setBudgetToDelete(null) })
        }
      />
      <ConfirmDialog
        name="rate-limit-delete"
        open={!!rateLimitToDelete}
        onOpenChange={(open) => {
          if (open) return;
          setRateLimitToDelete(null);
          removeRateLimit.reset();
        }}
        title={
          rateLimitToDelete
            ? t("pages.limits.confirm.rateLimitTitle", { limit: rateLimitCaps(rateLimitToDelete) })
            : ""
        }
        description={
          rateLimitToDelete ? t("pages.limits.confirm.rateLimitBody", { scope: scopeLabel }) : ""
        }
        confirmLabel={t("pages.limits.confirm.rateLimitConfirm")}
        pending={removeRateLimit.isPending}
        error={removeRateLimit.error}
        onConfirm={() =>
          rateLimitToDelete &&
          removeRateLimit.mutate(rateLimitToDelete.id, {
            onSuccess: () => setRateLimitToDelete(null),
          })
        }
      />
    </PageBody>
  );
}

/**
 * A budget's period in words: `monthly` for `30d`, `daily` for `1d`. A period
 * the dashboard has no name for is shown as it was stored, since guessing at
 * `7d` would put words in the operator's mouth (#1902).
 */
function usePeriodLabel(): (period: string) => string {
  const { t } = useTranslation();
  return (period) => {
    const kind = periodKind(period);
    return kind ? t(`pages.limits.periods.${kind}`) : period;
  };
}

/**
 * What tells one budget from another to a person: its cap, its window and its
 * scope. The card's controls and the delete confirmation name the row the same
 * way, so the dialog is recognisably about the card that opened it (#1214,
 * #1904). `scope` is the scope's name, since every card on the screen is the
 * picked scope's.
 */
function useBudgetNames(scope: string): (budget: BudgetRow) => {
  amount: string;
  period: string;
  scope: string;
} {
  // budgets are denominated in the deployment's settlement currency, not in
  // dollars — the `_usd` in the column name is historic (#1182)
  const fmt = useFormat();
  const currency = useCurrencyCode();
  const periodLabel = usePeriodLabel();
  return (budget) => ({
    amount: fmt.currency(Number(budget.limit_usd), currency),
    period: periodLabel(budget.period),
    scope,
  });
}

/**
 * A rate limit's caps as one phrase, `600 rpm · 150,000 tpm`. They are the only
 * thing that tells two limits on one scope apart, so the card's controls and
 * the delete confirmation both carry them (#1214, #1904).
 */
function useRateLimitCaps(): (limit: RateLimitRow) => string {
  const { t } = useTranslation();
  const fmt = useFormat();
  return (limit) =>
    [
      limit.rpm != null ? `${fmt.number(limit.rpm)} ${t("pages.limits.units.rpm")}` : null,
      limit.tpm != null ? `${fmt.number(limit.tpm)} ${t("pages.limits.units.tpm")}` : null,
    ]
      .filter(Boolean)
      .join(" · ") || t("pages.limits.noCaps");
}

/**
 * The frame both cards share: the cap as the figure, with the edit and delete
 * controls beside it on the first line.
 *
 * The controls sit in their own column rather than after the badges, so a card
 * whose badges wrap, like a budget with an unpriced override, keeps them at the
 * height every other card has them (#2095).
 */
function LimitCard({
  figure,
  actions,
  children,
}: {
  figure: React.ReactNode;
  actions: React.ReactNode;
  children?: React.ReactNode;
}) {
  return (
    <CardStack>
      <div className="flex items-start gap-2.5">
        <div className="min-w-0 flex-1">{figure}</div>
        <div className="flex shrink-0 items-center gap-1.5">{actions}</div>
      </div>
      {children}
    </CardStack>
  );
}

function BudgetCard({
  budget,
  scope,
  onEdit,
  onDelete,
  deleting,
  gateAt,
}: {
  budget: BudgetRow;
  scope: string;
  gateAt: RowScope | undefined;
  onEdit: () => void;
  onDelete: () => void;
  deleting: boolean;
}) {
  const { t } = useTranslation();
  const periodLabel = usePeriodLabel();
  // the gateway enforces a period it does not recognise as monthly, which a
  // budget stored before the control plane checked the value may still hold
  const recognised = periodKind(budget.period) !== null;
  // the label names the row: the grid is a wall of identical cards otherwise,
  // and "Delete budget" said three times tells a screen reader nothing (#1214)
  const names = useBudgetNames(scope)(budget);
  const label = t("pages.limits.deleteBudgetAria", names);
  const editLabel = t("pages.limits.editBudgetAria", names);
  return (
    <LimitCard
      figure={<span className="block truncate font-mono text-xl font-medium">{names.amount}</span>}
      actions={
        <RowCapabilityScope at={gateAt}>
          <RowIconButton
            gate="budget:update"
            control="budget-edit"
            title={editLabel}
            aria-label={editLabel}
            onClick={onEdit}
          >
            <Pencil className="h-3.5 w-3.5" />
          </RowIconButton>
          <DeleteIconButton
            gate="budget:delete"
            control="budget-delete"
            label={label}
            pending={deleting}
            onClick={onDelete}
          />
        </RowCapabilityScope>
      }
    >
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={recognised ? "outline" : "warning"}>{periodLabel(budget.period)}</Badge>
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
      </div>
      {/* the card is the one place the row's claim meets what the gateway does
          with it: a `7d` badge alone reads as a weekly cap (#1902) */}
      {!recognised && (
        <p className="flex items-start gap-1.5 text-xs text-[color:var(--status-warning-text)]">
          <AlertTriangle aria-hidden className="mt-px h-3.5 w-3.5 flex-none" />
          {t("pages.limits.periodUnrecognised", { period: budget.period })}
        </p>
      )}
    </LimitCard>
  );
}

function RateLimitCard({
  limit,
  scope,
  onEdit,
  onDelete,
  deleting,
  gateAt,
}: {
  limit: RateLimitRow;
  scope: string;
  gateAt: RowScope | undefined;
  onEdit: () => void;
  onDelete: () => void;
  deleting: boolean;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const caps = useRateLimitCaps()(limit);
  const label = t("pages.limits.deleteRateLimitAria", { limit: caps, scope });
  const editLabel = t("pages.limits.editRateLimitAria", { limit: caps, scope });
  return (
    <LimitCard
      // the caps are the card's figure, set the way the budget's amount is
      figure={
        <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1">
          {limit.rpm != null && (
            <CapFigure value={fmt.number(limit.rpm)} unit={t("pages.limits.units.rpm")} />
          )}
          {limit.tpm != null && (
            <CapFigure value={fmt.number(limit.tpm)} unit={t("pages.limits.units.tpm")} />
          )}
          {limit.rpm == null && limit.tpm == null && (
            <span className="text-sm leading-7 text-muted-foreground">
              {t("pages.limits.noCaps")}
            </span>
          )}
        </div>
      }
      actions={
        <RowCapabilityScope at={gateAt}>
          <RowIconButton
            gate="rate_limit:update"
            control="rate-limit-edit"
            title={editLabel}
            aria-label={editLabel}
            onClick={onEdit}
          >
            <Pencil className="h-3.5 w-3.5" />
          </RowIconButton>
          <DeleteIconButton
            gate="rate_limit:delete"
            control="rate-limit-delete"
            label={label}
            pending={deleting}
            onClick={onDelete}
          />
        </RowCapabilityScope>
      }
    />
  );
}

/** One cap of a rate limit: its number as the figure, its unit beside it. */
function CapFigure({ value, unit }: { value: string; unit: string }) {
  return (
    <span className="inline-flex items-baseline gap-1.5">
      <span className="font-mono text-xl font-medium">{value}</span>
      <span className="text-xs text-muted-foreground">{unit}</span>
    </span>
  );
}

/**
 * The budget form, for a new budget or, given `budget`, for that one in place
 * (#1285).
 *
 * An edit sends only the fields that moved. That keeps the `budget.update`
 * audit row to what the operator actually changed, and an untouched form has
 * nothing to send, so it cannot be saved.
 */
function BudgetSheet({
  open,
  onOpenChange,
  scopeType,
  scopeId,
  scopeName,
  budget,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  scopeType: string;
  scopeId: string;
  /** what the subtitle calls the scope: its name, not the `type:uuid` it is stored as */
  scopeName: string;
  budget?: BudgetRow;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  // the field is in the settlement currency the cards are formatted in, not in
  // dollars whatever the deployment settles in (#2095)
  const currency = useCurrencyCode();
  // a new budget opens on 100 / monthly; an edit opens on the row as it stands.
  // `limit_usd` arrives in the column's `numeric(12,4)` spelling, which reads
  // as 250.5 in a number field rather than 250.5000. a stored `30d` opens on
  // the window it is enforced as, monthly, and since nobody picked it again no
  // period is sent. one the gateway does not recognise opens on nothing: it is
  // enforced as monthly whatever the row says, so the form cannot be saved
  // until the operator picks the window it should count over (#1902)
  const seed = React.useMemo(
    () => ({
      limitUsd: budget ? String(Number(budget.limit_usd)) : "100",
      period: budget ? (periodKind(budget.period) ?? "") : "monthly",
      // "" is the inherit case, which is what the API means by a null override
      unpriced: (budget?.unpriced_policy ?? "") as UnpricedPolicy | "",
    }),
    [budget],
  );
  const [limitUsd, setLimitUsd] = React.useState(seed.limitUsd);
  const [period, setPeriod] = React.useState(seed.period);
  const [unpriced, setUnpriced] = React.useState<UnpricedPolicy | "">(seed.unpriced);

  React.useEffect(() => {
    if (open) {
      setLimitUsd(seed.limitUsd);
      setPeriod(seed.period);
      setUnpriced(seed.unpriced);
    }
  }, [open, seed]);

  const dirty = limitUsd !== seed.limitUsd || period !== seed.period || unpriced !== seed.unpriced;
  // the row being edited holds a period the gateway does not recognise, and no
  // window has been picked to replace it yet
  const unrecognised =
    budget !== undefined && periodKind(budget.period) === null && periodKind(period) === null;

  const save = useMutation({
    mutationFn: () => {
      if (budget) {
        const patch: UpdateBudgetInput = {};
        if (limitUsd !== seed.limitUsd) patch.limit_usd = limitUsd;
        if (period !== seed.period) patch.period = period;
        if (unpriced !== seed.unpriced) patch.unpriced_policy = unpriced === "" ? null : unpriced;
        return updateBudget(budget.id, patch);
      }
      return createBudget({
        scope_type: scopeType,
        scope_id: scopeId,
        limit_usd: limitUsd,
        period,
        unpriced_policy: unpriced === "" ? null : unpriced,
      });
    },
    onSuccess: () => {
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({
        tone: "success",
        title: t(budget ? "pages.limits.budgetUpdated" : "pages.limits.budgetCreated"),
      });
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
      name={budget ? "budget-edit" : "budget-create"}
      open={open}
      onOpenChange={onOpenChange}
      title={t(budget ? "pages.limits.budgetEditTitle" : "pages.limits.budgetSheetTitle")}
      subtitle={t("pages.limits.budgetSheetSubtitle", { scope: scopeName })}
      dirty={dirty}
      errorMessage={save.isError ? (save.error as Error).message : undefined}
      saveLabel={t(budget ? "common.save" : "common.create")}
      canSave={Boolean(limitUsd.trim() && periodKind(period)) && (!budget || dirty)}
      saving={save.isPending}
      onSave={() => save.mutate()}
    >
      <div className="space-y-3">
        <Field label={t("pages.limits.budgetLimitLabel", { currency })}>
          {/* the ceiling is the numeric(12,4) column's; the server refuses above it */}
          <Input
            type="number"
            min={0}
            max={99_999_999.99}
            step="0.01"
            value={limitUsd}
            onChange={(e) => setLimitUsd(e.target.value)}
          />
        </Field>
        {/* a picker rather than free text: the gateway has no rolling windows,
            and read any period it did not know as monthly, so the `7d` the old
            hint suggested was a calendar-month cap (#1902) */}
        <Field
          label={t("pages.limits.budgetPeriodLabel")}
          hint={
            unrecognised ? (
              <span className="flex items-start gap-1.5 text-[color:var(--status-warning-text)]">
                <AlertTriangle aria-hidden className="mt-px h-3.5 w-3.5 flex-none" />
                {t("pages.limits.periodUnrecognisedHint", { period: budget.period })}
              </span>
            ) : (
              t("pages.limits.budgetPeriodHint")
            )
          }
          htmlFor="budget-period"
        >
          <Combobox
            id="budget-period"
            value={period}
            onChange={setPeriod}
            placeholder={t("pages.limits.periodPlaceholder")}
            options={PERIOD_KINDS.map((kind) => ({
              value: kind,
              label: t(`pages.limits.periodOptions.${kind}`),
              description: t(`pages.limits.periodDescriptions.${kind}`),
            }))}
          />
        </Field>
        <Field
          label={t("pages.limits.unpricedLabel")}
          hint={t("pages.limits.unpricedHint")}
          htmlFor="budget-unpriced-policy"
        >
          <Combobox
            id="budget-unpriced-policy"
            value={unpriced}
            onChange={(picked) => setUnpriced(picked as UnpricedPolicy | "")}
            options={[
              { value: "", label: t("pages.limits.unpriced.inherit") },
              ...UNPRICED_POLICIES.map((policy) => ({
                value: policy,
                label: t(`pages.limits.unpriced.${policy}`),
              })),
            ]}
          />
        </Field>
      </div>
    </EditorSheet>
  );
}

/**
 * The rate-limit form, for a new limit or, given `limit`, for that one in place
 * (#1285). A blank field is an uncapped one either way: on an edit, blanking a
 * cap that was set sends `null`, which lifts it.
 */
function RateLimitSheet({
  open,
  onOpenChange,
  scopeType,
  scopeId,
  scopeName,
  limit,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  scopeType: string;
  scopeId: string;
  /** what the subtitle calls the scope: its name, not the `type:uuid` it is stored as */
  scopeName: string;
  limit?: RateLimitRow;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const seed = React.useMemo(
    () => ({
      rpm: limit?.rpm != null ? String(limit.rpm) : "",
      tpm: limit?.tpm != null ? String(limit.tpm) : "",
    }),
    [limit],
  );
  const [rpm, setRpm] = React.useState(seed.rpm);
  const [tpm, setTpm] = React.useState(seed.tpm);

  React.useEffect(() => {
    if (open) {
      setRpm(seed.rpm);
      setTpm(seed.tpm);
    }
  }, [open, seed]);

  const dirty = rpm !== seed.rpm || tpm !== seed.tpm;
  const cap = (value: string) => (value.trim() ? Number(value) : null);

  const save = useMutation({
    mutationFn: () => {
      if (limit) {
        const patch: UpdateRateLimitInput = {};
        if (rpm !== seed.rpm) patch.rpm = cap(rpm);
        if (tpm !== seed.tpm) patch.tpm = cap(tpm);
        return updateRateLimit(limit.id, patch);
      }
      return createRateLimit({
        scope_type: scopeType,
        scope_id: scopeId,
        rpm: cap(rpm) ?? undefined,
        tpm: cap(tpm) ?? undefined,
      });
    },
    onSuccess: () => {
      // the sheet closes on success, so the outcome is announced somewhere
      // that outlives it (#1197)
      toast.push({
        tone: "success",
        title: t(limit ? "pages.limits.rateLimitUpdated" : "pages.limits.rateLimitCreated"),
      });
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
      name={limit ? "rate-limit-edit" : "rate-limit-create"}
      open={open}
      onOpenChange={onOpenChange}
      title={t(limit ? "pages.limits.rateLimitEditTitle" : "pages.limits.rateLimitSheetTitle")}
      subtitle={t("pages.limits.rateLimitSheetSubtitle", { scope: scopeName })}
      dirty={dirty}
      errorMessage={save.isError ? (save.error as Error).message : undefined}
      saveLabel={t(limit ? "common.save" : "common.create")}
      canSave={Boolean(rpm.trim() || tpm.trim()) && (!limit || dirty)}
      saving={save.isPending}
      onSave={() => save.mutate()}
    >
      <div className="space-y-3">
        <Field label={t("pages.limits.rpmLabel")}>
          {/* the gateway reads a cap below 1 as none, so the server refuses one */}
          <Input
            type="number"
            min={1}
            step={1}
            value={rpm}
            onChange={(e) => setRpm(e.target.value)}
            placeholder={t("pages.limits.uncapped")}
          />
        </Field>
        <Field label={t("pages.limits.tpmLabel")}>
          <Input
            type="number"
            min={1}
            step={1}
            value={tpm}
            onChange={(e) => setTpm(e.target.value)}
            placeholder={t("pages.limits.uncapped")}
          />
        </Field>
      </div>
    </EditorSheet>
  );
}
