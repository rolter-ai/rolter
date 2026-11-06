import { useMutation, useQuery, useQueryClient, type UseQueryResult } from "@tanstack/react-query";
import { AlertTriangle, Gauge, Pencil, Plus, Wallet } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { LoadError } from "@/components/LoadError";
import { ControlSkeleton, ListSkeleton } from "@/components/LoadingState";
import { EditorSheet } from "@/components/EditorSheet";
import {
  ListActionsHeader,
  ListCell,
  ListEmptyRow,
  ListHeader,
  ListHeaderCell,
  ListLoadingRow,
  ListRow,
  ListTable,
  PageBody,
  RowIconButton,
  STICKY_ACTIONS,
  Toolbar,
  primaryColumn,
} from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Combobox, type ComboboxOption } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";

import {
  createBudget,
  createRateLimit,
  deleteBudget,
  deleteRateLimit,
  fetchBusinessUnits,
  fetchCustomers,
  fetchOrgBudgetSpend,
  fetchOrgBudgets,
  fetchOrgRateLimits,
  fetchOrgVirtualKeys,
  fetchUsers,
  SCOPE_TYPES,
  UNPRICED_POLICIES,
  USER_SCOPE,
  updateBudget,
  updateRateLimit,
  type BudgetRow,
  type BudgetSpendReport,
  type UnpricedPolicy,
  type RateLimitRow,
  type UpdateBudgetInput,
  type UpdateRateLimitInput,
  type UserScopeNarrowing,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { PERIOD_KINDS, periodKind } from "@/lib/budget-period";
import { spendFigures, type SpendTone } from "@/lib/budget-spend";
import { useCurrencyCode } from "@/lib/currency";
import { useFormat } from "@/lib/i18n/format";
import { useOptionalAuth } from "@/lib/auth";
import { useScope } from "@/lib/scope";
import { RowCapabilityScope, type RowScope } from "@/lib/can";
import { capGateScope, type CapRow } from "@/lib/limit-scope";
import { capReach, type CapReach } from "@/lib/my-budget";
import { useOrgScope } from "@/components/OrgScopePicker";
import { useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";
import { cn } from "@/lib/utils";

/** a person as the screen names them: what they go by, or the address they sign in with */
function personName(user: { display_name?: string | null; email: string }): string {
  return user.display_name?.trim() || user.email;
}

// the picker's placeholder per scope type, as catalog keys
const SCOPE_PLACEHOLDERS: Record<string, string> = {
  org: "pages.limits.selectOrg",
  team: "pages.limits.selectTeam",
  project: "pages.limits.selectProject",
  virtual_key: "pages.limits.selectVirtualKey",
  business_unit: "pages.limits.selectBusinessUnit",
  customer: "pages.limits.selectCustomer",
  user: "pages.limits.selectUser",
};

// the filter's entry for every scope type at once (#2093)
const ALL_SCOPES = "all";

// a row is found by its scope, so that is the first column; a budget then
// states its cap, the window it counts over and how much of it is spent
const BUDGET_GRID = `${primaryColumn(1.4)} 140px minmax(150px, 1fr) minmax(230px, 1.6fr) 72px`;
const RATE_LIMIT_GRID = `${primaryColumn(1.6)} minmax(150px, 1fr) minmax(150px, 1fr) 72px`;

/** a cap row as the org-wide reads answer it: a key's row also carries the key's name (#3128) */
type ListedCap = CapRow & { id: string; scope_name?: string | null };

/** how a row's scope is written: its name, what kind of scope it is, and both as one phrase */
interface ScopeLabel {
  name: string;
  detail: string;
  /** "Project default", for the controls' names and the delete confirmations */
  label: string;
}

/** the scope a new cap is created on */
interface ScopeDraft {
  type: string;
  id: string;
}

// every budget and rate limit the caller can read across the org, in two
// tables with the scope each one caps as a column (#3665). one filter narrows
// both, by scope type and then by one scope of it, and a link from another
// screen opens the page filtered to the scope it names. a new cap picks its
// scope in its sheet, so the page keeps no scope of its own beside the
// sidebar's
export default function Limits() {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const scope = useScope();
  const orgScope = useOrgScope(scope.orgId);
  // the scope hook names a catalog key rather than carrying english copy
  const scopeMessage = scope.errorKey ? t(scope.errorKey) : undefined;

  // a link from another screen can open the page on a scope, such as a team's
  // detail sheet: `?scope_type=team&scope_id=<id>`. an unknown type is ignored
  const [params] = useSearchParams();
  const linked = params.get("scope_type") ?? "";
  const fromLink = Object.prototype.hasOwnProperty.call(SCOPE_PLACEHOLDERS, linked);
  const [filterType, setFilterType] = React.useState<string>(fromLink ? linked : ALL_SCOPES);
  const [filterId, setFilterId] = React.useState<string>(
    fromLink ? (params.get("scope_id") ?? "") : "",
  );

  // `in-org` keeps these apart from the per-scope reads other screens make,
  // whose key is `["budgets", "org", <org id>]` when the scope is the org itself
  const budgets = useQuery({
    queryKey: ["budgets", "in-org", scope.orgId],
    queryFn: () => fetchOrgBudgets(scope.orgId as string),
    enabled: !!scope.orgId,
  });

  // the gateway's own counters, read through the control plane. a separate
  // query so a slow or failed read never holds the budgets themselves back
  const spend = useQuery({
    queryKey: ["budgets", "in-org", scope.orgId, "spend"],
    queryFn: () => fetchOrgBudgetSpend(scope.orgId as string),
    enabled: !!scope.orgId,
    refetchInterval: 30_000,
  });

  const rateLimits = useQuery({
    queryKey: ["rate-limits", "in-org", scope.orgId],
    queryFn: () => fetchOrgRateLimits(scope.orgId as string),
    enabled: !!scope.orgId,
  });

  // UX stream (#805): the lists are what the reader is waiting on
  useScreenReady(!budgets.isLoading && !rateLimits.isLoading);

  // the names are read only for the scope types the lists hold or the filter
  // asks for: an org with no customer caps never reads its customers
  const shown = new Set<string>([
    ...(budgets.data ?? []).map((row) => row.scope_type),
    ...(rateLimits.data ?? []).map((row) => row.scope_type),
  ]);
  if (filterType !== ALL_SCOPES) shown.add(filterType);
  const directory = useScopeDirectory(shown);

  // a cap written on one scope changes the org-wide list too, so both go stale
  const invalidateBudgets = () => queryClient.invalidateQueries({ queryKey: ["budgets"] });
  const invalidateRateLimits = () => queryClient.invalidateQueries({ queryKey: ["rate-limits"] });

  // a name is only unique within its type, so a row says which kind it is. a
  // user row names the person and what their cap counts, since the same person
  // can hold one cap per place it is narrowed to (#3282); until the people are
  // read it says only what it counts rather than showing a uuid
  const describe = (row: ListedCap): ScopeLabel => {
    const type = t(`pages.limits.scopeTypes.${row.scope_type}`);
    if (row.scope_type === USER_SCOPE) {
      const reach = t(`pages.limits.userScope.${capReach(row)}`);
      const person = directory.userName(row.scope_id);
      return {
        name: person ?? reach,
        detail: person ? `${type} · ${reach}` : type,
        label: t("pages.limits.scopeNamed", {
          type,
          name: person ? t("pages.limits.userScopeNamed", { name: person, reach }) : reach,
        }),
      };
    }
    const name = directory.nameOf(row.scope_type, row.scope_id, row.scope_name);
    return { name, detail: type, label: t("pages.limits.scopeNamed", { type, name }) };
  };

  // the filter narrows by type, then by one scope of it. a person's caps
  // narrowed to different places all belong to that person
  const filtered = filterType !== ALL_SCOPES;
  const matches = (row: ListedCap) =>
    (!filtered || row.scope_type === filterType) && (!filterId || row.scope_id === filterId);
  // scope types in the order the filter offers them, then scopes by name, so
  // "which caps exist here" reads scope by scope (#2093)
  const typeOrder = (row: ListedCap) => {
    const at = SCOPE_TYPES.indexOf(row.scope_type as (typeof SCOPE_TYPES)[number]);
    return at < 0 ? SCOPE_TYPES.length : at;
  };
  const listed = <T extends ListedCap>(rows: T[] | undefined): { row: T; scope: ScopeLabel }[] =>
    (rows ?? [])
      .filter(matches)
      .map((row) => ({ row, scope: describe(row) }))
      .sort(
        (a, b) => typeOrder(a.row) - typeOrder(b.row) || a.scope.label.localeCompare(b.scope.label),
      );
  const budgetRows = listed(budgets.data);
  const rateLimitRows = listed(rateLimits.data);

  // the second filter offers the scopes of the picked type that hold a cap,
  // plus the one a link named when it holds none, so the field is never blank
  const filterOptions = (() => {
    if (!filtered) return [];
    const found = new Map<string, string>();
    for (const row of [...(budgets.data ?? []), ...(rateLimits.data ?? [])]) {
      if (row.scope_type !== filterType || found.has(row.scope_id)) continue;
      found.set(row.scope_id, directory.nameOf(row.scope_type, row.scope_id, row.scope_name));
    }
    if (filterId && !found.has(filterId)) {
      found.set(filterId, directory.nameOf(filterType, filterId));
    }
    return [
      { value: "", label: t("pages.limits.filterAnyScope") },
      ...[...found]
        .map(([value, label]) => ({ value, label }))
        .sort((a, b) => a.label.localeCompare(b.label)),
    ];
  })();

  // a new cap opens on the scope the filter names, or on the sidebar's project
  const newCapScope: ScopeDraft = filtered
    ? { type: filterType, id: filterId || directory.defaultId(filterType) }
    : { type: "project", id: directory.defaultId("project") };

  // the chain a row's Edit and Delete are asked at: a key's caps are its
  // project's to manage, which the org-wide key list says (#2529)
  const gateOf = (row: ListedCap) =>
    capGateScope(row, {
      byTeam: orgScope.byTeam,
      keyProjectId: row.scope_type === "virtual_key" ? directory.keyProject(row.scope_id) : null,
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
        detail: errorText(error, t),
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
        detail: errorText(error, t),
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
  const budgetNames = useBudgetNames();
  const rateLimitCaps = useRateLimitCaps();
  const spendState: BudgetSpendReport | "loading" | null = spend.isLoading
    ? "loading"
    : spend.isError
      ? null
      : (spend.data ?? null);
  const typeName = t(`pages.limits.scopeTypes.${filterType}`);

  return (
    <PageBody className="gap-[22px]">
      {scopeBlocked && (
        <p className="text-sm text-muted-foreground">
          {t("pages.limits.scopeBlocked", { detail: scopeMessage })}
        </p>
      )}

      <Toolbar className="items-end">
        <Field
          label={t("pages.limits.scopeTypeLabel")}
          htmlFor="limits-filter-type"
          className="w-full sm:w-56"
        >
          <Combobox
            id="limits-filter-type"
            value={filterType}
            onChange={(picked) => {
              setFilterType(picked);
              setFilterId("");
            }}
            options={[ALL_SCOPES, ...SCOPE_TYPES].map((type) => ({
              value: type,
              label: t(`pages.limits.scopeTypes.${type}`),
            }))}
          />
        </Field>
        {filtered && (
          <Field
            label={t("pages.limits.scopeLabel")}
            htmlFor="limits-filter-scope"
            className="w-full sm:w-72"
          >
            <Combobox
              id="limits-filter-scope"
              value={filterId}
              onChange={setFilterId}
              options={filterOptions}
            />
          </Field>
        )}
      </Toolbar>

      <section className="space-y-3" aria-labelledby="limits-budgets">
        <div className="flex flex-wrap items-center gap-3">
          <div className="flex flex-col gap-0.5">
            <h2 id="limits-budgets" className="text-base font-medium">
              {t("pages.limits.budgetsTitle")}
            </h2>
            <span className="text-xs text-muted-foreground">{t("pages.limits.budgetsHint")}</span>
          </div>
          <GatedButton
            gate="budget:create"
            control="budget-new"
            size="sm"
            className="ml-auto"
            onClick={() => setAddBudgetOpen(true)}
            disabled={!scope.orgId}
          >
            <Plus className="h-4 w-4" aria-hidden />
            {t("pages.limits.budgetsAdd")}
          </GatedButton>
        </div>
        {budgets.error ? (
          <LoadError
            error={budgets.error}
            resource={t("errors.resources.budgets")}
            onRetry={() => budgets.refetch()}
            target="budgets"
          />
        ) : (
          <ListTable label={t("pages.limits.budgetsTitle")} minWidth={880}>
            <ListHeader grid={BUDGET_GRID}>
              <ListHeaderCell>{t("pages.limits.colScope")}</ListHeaderCell>
              <ListHeaderCell>{t("pages.limits.colCap")}</ListHeaderCell>
              <ListHeaderCell>{t("pages.limits.colPeriod")}</ListHeaderCell>
              <ListHeaderCell>{t("pages.limits.colSpend")}</ListHeaderCell>
              <ListActionsHeader className={STICKY_ACTIONS.header} />
            </ListHeader>
            <ListLoadingRow read={budgets}>
              <ListSkeleton rows={3} className="p-3" />
            </ListLoadingRow>
            <ListEmptyRow read={budgets} rows={budgetRows.length}>
              <EmptyState
                uxTarget="budgets"
                icon={<Wallet />}
                title={t(
                  filterId
                    ? "pages.limits.budgetsEmptyTitle"
                    : filtered
                      ? "pages.limits.budgetsEmptyTypeTitle"
                      : "pages.limits.budgetsEmptyAllTitle",
                )}
                description={t(
                  filterId
                    ? "pages.limits.budgetsEmptyBody"
                    : filtered
                      ? "pages.limits.budgetsEmptyTypeBody"
                      : "pages.limits.budgetsEmptyAllBody",
                  { type: typeName },
                )}
                actions={
                  <GatedButton
                    gate="budget:create"
                    control="budget-new-empty"
                    onClick={() => setAddBudgetOpen(true)}
                  >
                    {t("pages.limits.budgetsEmptyAction")}
                  </GatedButton>
                }
              />
            </ListEmptyRow>
            {budgetRows.map(({ row, scope: label }) => (
              <BudgetLine
                key={row.id}
                budget={row}
                scope={label}
                spend={spendState}
                gateAt={gateOf(row)}
                onEdit={() => {
                  setEditingBudget(row);
                  setEditBudgetOpen(true);
                }}
                onDelete={() => setBudgetToDelete(row)}
                // the mutation is shared by every row, so only the one it was
                // given spins
                deleting={removeBudget.isPending && budgetToDelete?.id === row.id}
              />
            ))}
          </ListTable>
        )}
      </section>

      <section className="space-y-3" aria-labelledby="limits-rate-limits">
        <div className="flex flex-wrap items-center gap-3">
          <div className="flex flex-col gap-0.5">
            <h2 id="limits-rate-limits" className="text-base font-medium">
              {t("pages.limits.rateLimitsTitle")}
            </h2>
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
            disabled={!scope.orgId}
          >
            <Plus className="h-4 w-4" aria-hidden />
            {t("pages.limits.rateLimitsAdd")}
          </GatedButton>
        </div>
        {rateLimits.error ? (
          <LoadError
            error={rateLimits.error}
            resource={t("errors.resources.rateLimits")}
            onRetry={() => rateLimits.refetch()}
            target="rate-limits"
          />
        ) : (
          <ListTable label={t("pages.limits.rateLimitsTitle")} minWidth={620}>
            <ListHeader grid={RATE_LIMIT_GRID}>
              <ListHeaderCell>{t("pages.limits.colScope")}</ListHeaderCell>
              <ListHeaderCell>{t("pages.limits.colRpm")}</ListHeaderCell>
              <ListHeaderCell>{t("pages.limits.colTpm")}</ListHeaderCell>
              <ListActionsHeader className={STICKY_ACTIONS.header} />
            </ListHeader>
            <ListLoadingRow read={rateLimits}>
              <ListSkeleton rows={3} className="p-3" />
            </ListLoadingRow>
            <ListEmptyRow read={rateLimits} rows={rateLimitRows.length}>
              <EmptyState
                uxTarget="rate-limits"
                icon={<Gauge />}
                title={t(
                  filterId
                    ? "pages.limits.rateLimitsEmptyTitle"
                    : filtered
                      ? "pages.limits.rateLimitsEmptyTypeTitle"
                      : "pages.limits.rateLimitsEmptyAllTitle",
                )}
                description={t(
                  filterId
                    ? "pages.limits.rateLimitsEmptyBody"
                    : filtered
                      ? "pages.limits.rateLimitsEmptyTypeBody"
                      : "pages.limits.rateLimitsEmptyAllBody",
                  { type: typeName },
                )}
                actions={
                  <GatedButton
                    gate="rate_limit:create"
                    control="rate-limit-new-empty"
                    onClick={() => setAddRateLimitOpen(true)}
                  >
                    {t("pages.limits.rateLimitsEmptyAction")}
                  </GatedButton>
                }
              />
            </ListEmptyRow>
            {rateLimitRows.map(({ row, scope: label }) => (
              <RateLimitLine
                key={row.id}
                limit={row}
                scope={label}
                gateAt={gateOf(row)}
                onEdit={() => {
                  setEditingRateLimit(row);
                  setEditRateLimitOpen(true);
                }}
                onDelete={() => setRateLimitToDelete(row)}
                deleting={removeRateLimit.isPending && rateLimitToDelete?.id === row.id}
              />
            ))}
          </ListTable>
        )}
      </section>

      <BudgetSheet
        open={addBudgetOpen}
        onOpenChange={setAddBudgetOpen}
        target={newCapScope}
        onDone={invalidateBudgets}
      />
      {editingBudget && (
        <BudgetSheet
          open={editBudgetOpen}
          onOpenChange={setEditBudgetOpen}
          scopeName={describe(editingBudget).label}
          budget={editingBudget}
          onDone={invalidateBudgets}
        />
      )}
      <RateLimitSheet
        open={addRateLimitOpen}
        onOpenChange={setAddRateLimitOpen}
        target={newCapScope}
        onDone={invalidateRateLimits}
      />
      {editingRateLimit && (
        <RateLimitSheet
          open={editRateLimitOpen}
          onOpenChange={setEditRateLimitOpen}
          scopeName={describe(editingRateLimit).label}
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
          budgetToDelete
            ? t(
                "pages.limits.confirm.budgetTitle",
                budgetNames(budgetToDelete, describe(budgetToDelete).label),
              )
            : ""
        }
        description={
          budgetToDelete
            ? t("pages.limits.confirm.budgetBody", { scope: describe(budgetToDelete).label })
            : ""
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
          rateLimitToDelete
            ? t("pages.limits.confirm.rateLimitBody", { scope: describe(rateLimitToDelete).label })
            : ""
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

/** the read behind one scope type's names, which a picker waits on and says it failed */
interface ScopeRead {
  query: UseQueryResult<unknown, Error>;
  target: string;
  resource: string;
}

/**
 * The scopes a cap can hang off, by name: what the filter and the sheet offer,
 * and what a row is called. Orgs, teams and projects come from the scope
 * hooks every screen shares; keys, business units, customers and people are
 * read only for the types in `types`. A key's org-wide row also names it
 * (`scope_name`, #3128), and the org-wide key list says which project a key's
 * caps are managed in.
 */
function useScopeDirectory(types: ReadonlySet<string>) {
  const scope = useScope();
  const orgScope = useOrgScope(scope.orgId);
  const orgId = scope.orgId;

  const keys = useQuery({
    queryKey: ["virtual-keys", "org", orgId],
    queryFn: () => fetchOrgVirtualKeys(orgId as string),
    enabled: !!orgId && types.has("virtual_key"),
  });
  // the two governance dimensions a key's spend rolls up to (#539)
  const businessUnits = useQuery({
    queryKey: ["business-units", orgId],
    queryFn: () => fetchBusinessUnits(orgId as string),
    enabled: !!orgId && types.has("business_unit"),
  });
  const customers = useQuery({
    queryKey: ["customers", orgId],
    queryFn: () => fetchCustomers(orgId as string),
    enabled: !!orgId && types.has("customer"),
  });
  // the people a user cap can be set on (#3282)
  const users = useQuery({
    queryKey: ["users", orgId],
    queryFn: () => fetchUsers(orgId as string),
    enabled: !!orgId && types.has(USER_SCOPE),
  });

  const named = (rows: { id: string; name: string }[] | undefined): ComboboxOption[] =>
    (rows ?? []).map((row) => ({ value: row.id, label: row.name }));
  const options = (type: string): ComboboxOption[] => {
    switch (type) {
      case "org":
        return named(scope.orgs);
      case "team":
        return named(orgScope.teams);
      case "project":
        return named(orgScope.byTeam.flatMap((entry) => entry.projects));
      case "virtual_key":
        return (keys.data ?? []).map((key) => ({
          value: key.id,
          label: key.name || key.key_prefix,
        }));
      case "business_unit":
        return named(businessUnits.data);
      case "customer":
        return named(customers.data);
      case USER_SCOPE:
        return (users.data ?? []).map((user) => ({
          value: user.id,
          label: personName(user),
          description: user.display_name ? user.email : undefined,
        }));
      default:
        return [];
    }
  };
  const read = (type: string): ScopeRead | null => {
    switch (type) {
      case "virtual_key":
        return { query: keys, target: "scope-virtual-keys", resource: "virtualKeys" };
      case "business_unit":
        return { query: businessUnits, target: "scope-business-units", resource: "businessUnits" };
      case "customer":
        return { query: customers, target: "scope-customers", resource: "customers" };
      case USER_SCOPE:
        return { query: users, target: "scope-users", resource: "users" };
      default:
        return null;
    }
  };
  // a type whose names are still on their way, so its picker is not drawn empty
  const pending = (type: string) =>
    type === "org"
      ? scope.isLoading
      : type === "team" || type === "project"
        ? scope.isLoading || orgScope.isLoading
        : !!read(type)?.query.isLoading;
  const userName = (id: string) => {
    const user = users.data?.find((entry) => entry.id === id);
    return user ? personName(user) : undefined;
  };
  // a scope the lists cannot name is shown by its id, a key's by the start of it
  const nameOf = (type: string, id: string, rowName?: string | null): string => {
    if (type === "virtual_key" && rowName) return rowName;
    const found = options(type).find((option) => option.value === id)?.label;
    return found || (type === "virtual_key" ? id.slice(0, 8) : id);
  };
  const keyProject = (id: string) => keys.data?.find((key) => key.id === id)?.project_id ?? null;
  // where a new cap of a type opens: the sidebar's org, team or project
  const sidebar: Record<string, string | undefined> = {
    org: scope.orgId,
    team: scope.teamId,
    project: scope.projectId,
  };
  const defaultId = (type: string) => sidebar[type] ?? "";
  return { options, read, pending, userName, nameOf, keyProject, defaultId };
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
 * scope. The row's controls and the delete confirmation name the row the same
 * way, so the dialog is recognisably about the row that opened it (#1214,
 * #1904). `scope` is the scope as the row writes it, kind and name.
 */
function useBudgetNames(): (
  budget: BudgetRow,
  scope: string,
) => {
  amount: string;
  period: string;
  scope: string;
} {
  // budgets are denominated in the deployment's settlement currency, not in
  // dollars — the `_usd` in the column name is historic (#1182)
  const fmt = useFormat();
  const currency = useCurrencyCode();
  const periodLabel = usePeriodLabel();
  return (budget, scope) => ({
    amount: fmt.currency(Number(budget.limit_usd), currency),
    period: periodLabel(budget.period),
    scope,
  });
}

/**
 * A rate limit's caps as one phrase, `600 rpm · 150,000 tpm`. They are the only
 * thing that tells two limits on one scope apart, so the row's controls and
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

const SPEND_BAR_TONE: Record<SpendTone, string> = {
  ok: "bg-primary",
  warning: "bg-[color:var(--status-warning)]",
  danger: "bg-[color:var(--status-danger)]",
};

const SPEND_TEXT_TONE: Record<SpendTone, string> = {
  ok: "text-muted-foreground",
  warning: "text-[color:var(--status-warning-text)]",
  danger: "text-[color:var(--status-danger-text)]",
};

/**
 * Spent against the cap, as figures and a bar, with the reset time (#2091).
 *
 * The counter is the one the gateway enforces. When the control plane could not
 * read it the row says so; a missing figure is never drawn as an empty bar,
 * because that is what an untouched budget looks like.
 */
function BudgetSpend({
  budget,
  spend,
}: {
  budget: BudgetRow;
  spend: BudgetSpendReport | "loading" | null;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const currency = useCurrencyCode();
  if (spend === "loading") return null;
  const row = spend?.budgets.find((entry) => entry.id === budget.id);
  const figures = spendFigures(row, budget.limit_usd);
  if (!figures) {
    return (
      <p className="text-xs text-muted-foreground" data-spend="unavailable">
        {spend?.status === "not_configured"
          ? t("pages.limits.spend.notConfigured")
          : t("pages.limits.spend.unavailable")}
      </p>
    );
  }
  const percent = fmt.percent(figures.ratio, 0);
  const money = (value: number) => fmt.currency(value, currency);
  return (
    <div className="space-y-1.5" data-spend={figures.tone}>
      <div
        role="progressbar"
        aria-label={t("pages.limits.spend.barLabel")}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(figures.fraction * 100)}
        aria-valuetext={t("pages.limits.spend.used", { percent })}
        className="h-1.5 w-full overflow-hidden rounded-full bg-muted"
      >
        <div
          className={`h-full rounded-full ${SPEND_BAR_TONE[figures.tone]}`}
          style={{ width: `${figures.fraction * 100}%` }}
        />
      </div>
      <p className={`text-xs ${SPEND_TEXT_TONE[figures.tone]}`}>
        {figures.tone === "danger"
          ? t("pages.limits.spend.exhausted", { spent: money(figures.spent), percent })
          : t("pages.limits.spend.summary", {
              spent: money(figures.spent),
              remaining: money(figures.remaining),
              percent,
            })}
      </p>
      {row?.resets_at && (
        <p className="text-xs text-muted-foreground">
          {t("pages.limits.spend.resets", { when: fmt.dateTime(row.resets_at) })}
        </p>
      )}
    </div>
  );
}

/** A row's scope: its name, with the kind of scope under it. */
function ScopeCell({ scope }: { scope: ScopeLabel }) {
  return (
    <ListCell className="flex min-w-0 flex-col">
      <span className="truncate text-sm font-medium">{scope.name}</span>
      <span className="truncate text-xs text-muted-foreground">{scope.detail}</span>
    </ListCell>
  );
}

/** A row's edit and delete, asked at the chain the row's own scope sits in (#2529). */
function CapActions({
  gateAt,
  children,
}: {
  gateAt: RowScope | undefined;
  children: React.ReactNode;
}) {
  return (
    // stretched to the row's height below `md`, where the cell is pinned over
    // the columns scrolling under it and a two-line cap would show above and
    // below a cell only as tall as its buttons
    <ListCell
      className={cn(
        "flex items-center justify-end gap-[5px]",
        STICKY_ACTIONS.cell,
        "max-md:self-stretch",
      )}
    >
      <RowCapabilityScope at={gateAt}>{children}</RowCapabilityScope>
    </ListCell>
  );
}

function BudgetLine({
  budget,
  scope,
  spend,
  onEdit,
  onDelete,
  deleting,
  gateAt,
}: {
  budget: BudgetRow;
  scope: ScopeLabel;
  /**
   * "loading" while the read is in flight, null when it failed outright
   */
  spend: BudgetSpendReport | "loading" | null;
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
  // the controls name the row: "Delete budget" said once per row tells a screen
  // reader nothing (#1214)
  const names = useBudgetNames()(budget, scope.label);
  const editLabel = t("pages.limits.editBudgetAria", names);
  return (
    <ListRow grid={BUDGET_GRID} data-budget={budget.id}>
      <ScopeCell scope={scope} />
      <ListCell className="flex min-w-0 flex-col items-start gap-1">
        <span className="font-mono text-sm font-medium">{names.amount}</span>
        {/* an override is worth showing because it changes what the gateway
            will serve, not just what it counts (#996). a budget without one
            inherits the deployment setting and says nothing */}
        {budget.unpriced_policy && (
          <Badge tone={budget.unpriced_policy === "block" ? "danger" : "warning"}>
            {t("pages.limits.unpricedBadge", {
              policy: t(`pages.limits.unpriced.${budget.unpriced_policy}`),
            })}
          </Badge>
        )}
      </ListCell>
      <ListCell className="flex min-w-0 flex-col items-start gap-1.5">
        <Badge tone={recognised ? "outline" : "warning"}>{periodLabel(budget.period)}</Badge>
        {/* the row is the one place the stored claim meets what the gateway
            does with it: a `7d` badge alone reads as a weekly cap (#1902) */}
        {!recognised && (
          <p className="flex items-start gap-1.5 text-xs text-[color:var(--status-warning-text)]">
            <AlertTriangle aria-hidden className="mt-px h-3.5 w-3.5 flex-none" />
            {t("pages.limits.periodUnrecognised", { period: budget.period })}
          </p>
        )}
      </ListCell>
      <ListCell className="min-w-0">
        <BudgetSpend budget={budget} spend={spend} />
      </ListCell>
      <CapActions gateAt={gateAt}>
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
          label={t("pages.limits.deleteBudgetAria", names)}
          pending={deleting}
          onClick={onDelete}
        />
      </CapActions>
    </ListRow>
  );
}

function RateLimitLine({
  limit,
  scope,
  onEdit,
  onDelete,
  deleting,
  gateAt,
}: {
  limit: RateLimitRow;
  scope: ScopeLabel;
  gateAt: RowScope | undefined;
  onEdit: () => void;
  onDelete: () => void;
  deleting: boolean;
}) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const caps = useRateLimitCaps()(limit);
  const editLabel = t("pages.limits.editRateLimitAria", { limit: caps, scope: scope.label });
  // a blank cap is no cap on that measure, said in words rather than as a dash
  const cap = (value: number | null | undefined) =>
    value != null ? (
      <span className="font-mono text-sm font-medium">{fmt.number(value)}</span>
    ) : (
      <span className="text-sm text-muted-foreground">{t("pages.limits.uncapped")}</span>
    );
  return (
    <ListRow grid={RATE_LIMIT_GRID} data-rate-limit={limit.id}>
      <ScopeCell scope={scope} />
      <ListCell>{cap(limit.rpm)}</ListCell>
      <ListCell>{cap(limit.tpm)}</ListCell>
      <CapActions gateAt={gateAt}>
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
          label={t("pages.limits.deleteRateLimitAria", { limit: caps, scope: scope.label })}
          pending={deleting}
          onClick={onDelete}
        />
      </CapActions>
    </ListRow>
  );
}

/** the places a user cap can be narrowed to, as the sheet's pickers list them */
interface UserReachOptions {
  orgs: { value: string; label: string }[];
  projects: { value: string; label: string }[];
  /** the org the picker opens on, so the common case is one click */
  orgId: string | null;
  /** only a superadmin may leave a cap un-narrowed (#1830) */
  canLeaveOpen: boolean;
}

interface ReachDraft {
  reach: CapReach;
  id: string;
}

const initialReach = (options: UserReachOptions | undefined): ReachDraft => ({
  reach: "org",
  id: options?.orgId ?? "",
});

/** the narrowing a draft sends; an un-narrowed cap sends none */
function withinOf(draft: ReachDraft): UserScopeNarrowing {
  if (draft.reach === "org") return { within_org_id: draft.id };
  if (draft.reach === "project") return { within_project_id: draft.id };
  return {};
}

const reachComplete = (draft: ReachDraft) => draft.reach === "all" || draft.id !== "";

/**
 * Which of a person's keys a new cap counts (#3282): all of them, the ones
 * minted in one organization, or the ones minted in one project. The place also
 * decides who may manage the cap, so an un-narrowed one is offered only to a
 * superadmin.
 */
function UserReachFields({
  value,
  onChange,
  options,
}: {
  value: ReachDraft;
  onChange: (next: ReachDraft) => void;
  options: UserReachOptions;
}) {
  const { t } = useTranslation();
  const reaches: CapReach[] = options.canLeaveOpen ? ["org", "project", "all"] : ["org", "project"];
  const places = value.reach === "project" ? options.projects : options.orgs;
  return (
    <>
      <Field
        label={t("pages.limits.reachLabel")}
        hint={t("pages.limits.reachHint")}
        htmlFor="cap-reach"
      >
        <Combobox
          id="cap-reach"
          value={value.reach}
          onChange={(reach) =>
            onChange({
              reach: reach as CapReach,
              id: reach === "org" ? (options.orgId ?? "") : "",
            })
          }
          options={reaches.map((reach) => ({
            value: reach,
            label: t(`pages.limits.reachOptions.${reach}`),
          }))}
        />
      </Field>
      {value.reach !== "all" && (
        <Field
          label={t(`pages.limits.reachPlace.${value.reach}`)}
          htmlFor="cap-reach-place"
          required
        >
          <Combobox
            id="cap-reach-place"
            value={value.id}
            onChange={(id) => onChange({ ...value, id })}
            placeholder={t(`pages.limits.reachPlacePlaceholder.${value.reach}`)}
            options={places}
          />
        </Field>
      )}
    </>
  );
}

/** where a new user cap can be narrowed to, which is also who may manage it (#3282) */
function useUserReachOptions(): UserReachOptions {
  const scope = useScope();
  const orgScope = useOrgScope(scope.orgId);
  const auth = useOptionalAuth();
  return {
    orgs: scope.orgs.map((org) => ({ value: org.id, label: org.name })),
    projects: orgScope.byTeam.flatMap((entry) =>
      entry.projects.map((project) => ({ value: project.id, label: project.name })),
    ),
    orgId: scope.orgId ?? null,
    // an open-mode session has no account, and its caller acts as the admin
    // token, so it may leave a cap un-narrowed; the control plane decides
    canLeaveOpen: auth?.user ? auth.user.is_superadmin : true,
  };
}

type ScopeDirectory = ReturnType<typeof useScopeDirectory>;

/**
 * Where a new cap goes (#3665): a scope type, then one scope of it, picked by
 * name wherever the dashboard can list that type and pasted as a uuid where it
 * cannot (#1202). A type whose names are still being read shows a skeleton
 * rather than an empty picker, and one whose read failed says so beside the
 * field, since the picker falls back to the uuid box without it (#2642, #2959).
 */
function CapScopeFields({
  value,
  onChange,
  directory,
}: {
  value: ScopeDraft;
  onChange: (next: ScopeDraft) => void;
  directory: ScopeDirectory;
}) {
  const { t } = useTranslation();
  const options = directory.options(value.type);
  const read = directory.read(value.type);
  const pending = directory.pending(value.type);
  return (
    <>
      <Field
        label={t("pages.limits.scopeTypeLabel")}
        info={t("pages.limits.scopeTypeInfo")}
        htmlFor="cap-scope-type"
      >
        <Combobox
          id="cap-scope-type"
          value={value.type}
          onChange={(type) => onChange({ type, id: directory.defaultId(type) })}
          options={SCOPE_TYPES.map((type) => ({
            value: type,
            label: t(`pages.limits.scopeTypes.${type}`),
          }))}
        />
      </Field>
      <Field
        label={t("pages.limits.scopeLabel")}
        htmlFor="cap-scope"
        required
        hint={
          pending || options.length > 0
            ? undefined
            : t("pages.limits.scopeIdHint", { type: t(`pages.limits.scopeTypes.${value.type}`) })
        }
      >
        {pending ? (
          <ControlSkeleton width="100%" />
        ) : options.length > 0 ? (
          <Combobox
            id="cap-scope"
            value={value.id}
            onChange={(id) => onChange({ ...value, id })}
            placeholder={t(SCOPE_PLACEHOLDERS[value.type])}
            options={options}
          />
        ) : (
          <Input
            id="cap-scope"
            value={value.id}
            onChange={(e) => onChange({ ...value, id: e.target.value })}
            placeholder="00000000-0000-0000-0000-000000000000"
            className="font-mono text-xs"
          />
        )}
      </Field>
      {read?.query.error && (
        // load-error-allow: the scope picker's options; a picker has no empty state of its own
        <LoadError
          error={read.query.error}
          resource={t(`errors.resources.${read.resource}`)}
          onRetry={() => read.query.refetch()}
          target={read.target}
        />
      )}
    </>
  );
}

/**
 * The state a create sheet keeps for the scope it puts a new cap on, reset to
 * `target` each time it opens, with what its subtitle calls that scope.
 */
function useNewCapScope(open: boolean, target: ScopeDraft | undefined) {
  const { t } = useTranslation();
  const [where, setWhere] = React.useState<ScopeDraft>(() => target ?? { type: "project", id: "" });
  // until the reader picks a scope the sheet follows `target`, which fills in
  // once the sidebar's project has loaded: a sheet opened a moment earlier
  // would otherwise keep the empty scope it opened on
  const [picked, setPicked] = React.useState(false);
  const reachOptions = useUserReachOptions();
  const [reach, setReach] = React.useState<ReachDraft>(() => initialReach(reachOptions));
  React.useEffect(() => {
    if (!open) return;
    setPicked(false);
    setReach(initialReach(reachOptions));
    // the options are rebuilt every render; the sheet resets when it opens
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
  const targetType = target?.type;
  const targetId = target?.id;
  React.useEffect(() => {
    if (open && targetType !== undefined && !picked) {
      setWhere({ type: targetType, id: targetId ?? "" });
    }
  }, [open, picked, targetType, targetId]);
  const directory = useScopeDirectory(new Set([where.type]));
  const isUser = where.type === USER_SCOPE;
  return {
    where,
    setWhere: (next: ScopeDraft) => {
      setPicked(true);
      setWhere(next);
    },
    reach,
    setReach,
    reachOptions,
    directory,
    isUser,
    complete: where.id.trim() !== "" && (!isUser || reachComplete(reach)),
    /** what a new cap sends for its scope, a person's narrowing included */
    body: () => ({
      scope_type: where.type,
      scope_id: where.id.trim(),
      ...(isUser ? withinOf(reach) : {}),
    }),
    named: where.id
      ? t("pages.limits.scopeNamed", {
          type: t(`pages.limits.scopeTypes.${where.type}`),
          name: directory.nameOf(where.type, where.id),
        })
      : "",
  };
}

/** a create sheet's scope fields, and a person's narrowing when the scope is one */
function NewCapScopeFields({ creating }: { creating: ReturnType<typeof useNewCapScope> }) {
  return (
    <>
      <CapScopeFields
        value={creating.where}
        onChange={creating.setWhere}
        directory={creating.directory}
      />
      {creating.isUser && (
        <UserReachFields
          value={creating.reach}
          onChange={creating.setReach}
          options={creating.reachOptions}
        />
      )}
    </>
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
  target,
  scopeName = "",
  budget,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** where a new budget opens; the sheet asks for its scope (#3665) */
  target?: ScopeDraft;
  /** what an edit's subtitle calls the scope: its kind and name, not the `type:uuid` it is stored as */
  scopeName?: string;
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
  const creating = useNewCapScope(open, budget ? undefined : target);

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
        ...creating.body(),
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
        detail: errorText(error, t),
      });
    },
  });

  return (
    <EditorSheet
      name={budget ? "budget-edit" : "budget-create"}
      open={open}
      onOpenChange={onOpenChange}
      title={t(budget ? "pages.limits.budgetEditTitle" : "pages.limits.budgetSheetTitle")}
      subtitle={
        budget || creating.named
          ? t("pages.limits.budgetSheetSubtitle", { scope: budget ? scopeName : creating.named })
          : t("pages.limits.budgetSheetSubtitleNew")
      }
      dirty={dirty}
      errorMessage={save.isError ? errorText(save.error, t) : undefined}
      saveLabel={t(budget ? "common.save" : "common.create")}
      canSave={
        Boolean(limitUsd.trim() && periodKind(period)) && (budget ? dirty : creating.complete)
      }
      saving={save.isPending}
      onSubmit={() => save.mutate()}
    >
      <div className="space-y-3">
        {!budget && <NewCapScopeFields creating={creating} />}
        <Field
          label={t("pages.limits.budgetLimitLabel", { currency })}
          info={t("pages.limits.budgetLimitInfo", { currency })}
          required
        >
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
  target,
  scopeName = "",
  limit,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** where a new limit opens; the sheet asks for its scope (#3665) */
  target?: ScopeDraft;
  /** what an edit's subtitle calls the scope: its kind and name, not the `type:uuid` it is stored as */
  scopeName?: string;
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
  const creating = useNewCapScope(open, limit ? undefined : target);

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
        ...creating.body(),
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
        detail: errorText(error, t),
      });
    },
  });

  return (
    <EditorSheet
      name={limit ? "rate-limit-edit" : "rate-limit-create"}
      open={open}
      onOpenChange={onOpenChange}
      title={t(limit ? "pages.limits.rateLimitEditTitle" : "pages.limits.rateLimitSheetTitle")}
      subtitle={
        limit || creating.named
          ? t("pages.limits.rateLimitSheetSubtitle", { scope: limit ? scopeName : creating.named })
          : t("pages.limits.rateLimitSheetSubtitleNew")
      }
      dirty={dirty}
      errorMessage={save.isError ? errorText(save.error, t) : undefined}
      saveLabel={t(limit ? "common.save" : "common.create")}
      canSave={Boolean(rpm.trim() || tpm.trim()) && (limit ? dirty : creating.complete)}
      // either cap will do, so neither field is `required` on its own: a refused
      // submit with both empty says so in the footer instead
      blockedMessage={rpm.trim() || tpm.trim() ? undefined : t("pages.limits.rateLimitNeedsOne")}
      saving={save.isPending}
      onSubmit={() => save.mutate()}
    >
      <div className="space-y-3">
        {!limit && <NewCapScopeFields creating={creating} />}
        <Field label={t("pages.limits.rpmLabel")} info={t("pages.limits.rpmInfo")}>
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
        <Field label={t("pages.limits.tpmLabel")} info={t("pages.limits.tpmInfo")}>
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
