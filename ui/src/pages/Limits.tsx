import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Gauge, Pencil, Plus, Wallet } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton, ControlSkeleton } from "@/components/LoadingState";
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
  fetchBudgetSpend,
  fetchOrgBudgetSpend,
  fetchBudgets,
  fetchBusinessUnits,
  fetchCustomers,
  fetchOrgBudgets,
  fetchOrgRateLimits,
  fetchRateLimits,
  fetchUsers,
  fetchVirtualKeys,
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
import { capGateScope } from "@/lib/limit-scope";
import { capReach, type CapReach } from "@/lib/my-budget";
import { useOrgScope } from "@/components/OrgScopePicker";
import { useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";

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

// the scope type that lists every cap the caller may read across the org
// instead of the caps of one scope type (#2093)
const ALL_SCOPES = "all";

// budgets and rate limits share a scope (scope_type + scope_id), so this
// page combines both concerns behind one scope picker. it opens on the current
// project. the type narrows the org-wide list and a picked scope narrows it
// to one scope, which is also the only place a cap is created: a new cap needs
// a scope to hang off.
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
  const linkedId = params.get("scope_id") ?? "";
  const fromLink = Object.prototype.hasOwnProperty.call(SCOPE_PLACEHOLDERS, linked) && !!linkedId;
  const [scopeType, setScopeType] = React.useState<string>(fromLink ? linked : "project");
  const [scopeId, setScopeId] = React.useState<string>(fromLink ? linkedId : "");

  // the screen opens on the current project, once: choosing a scope type later
  // leaves the scope unpicked, which lists that type across the org (#2093)
  const [typePicked, setTypePicked] = React.useState(false);
  React.useEffect(() => {
    if (typePicked || scopeId || !scope.projectId) return;
    setScopeId(scope.projectId);
  }, [typePicked, scopeId, scope.projectId]);

  // no scope picked: the lists come from the org-wide read, narrowed by the
  // type. not while the project default is still on its way, which would read
  // the whole org for the first frame and then the project
  const defaultPending = !typePicked && !scopeId && (scope.isLoading || !!scope.projectId);
  const orgWide = !scopeId && !defaultPending;
  // a person's caps come from the org-wide read too: the per-scope read of a user
  // is authorized at the exact place a cap is narrowed to, which a bare person
  // does not name. the read is narrowed to the person below (#3282)
  const userScope = scopeType === USER_SCOPE;
  const orgRead = orgWide || userScope;

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
    enabled:
      !!scope.orgId && (scopeType === "business_unit" || (orgWide && scopeType === ALL_SCOPES)),
  });

  const customers = useQuery({
    queryKey: ["customers", scope.orgId],
    queryFn: () => fetchCustomers(scope.orgId as string),
    enabled: !!scope.orgId && (scopeType === "customer" || (orgWide && scopeType === ALL_SCOPES)),
  });

  // the people a user cap can be set on, and the names a user row is drawn with
  // everywhere it appears (#3282)
  const users = useQuery({
    queryKey: ["users", scope.orgId],
    queryFn: () => fetchUsers(scope.orgId as string),
    enabled: !!scope.orgId && (userScope || (orgWide && scopeType === ALL_SCOPES)),
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `virtualKeys` is the query the user is actually waiting on for this screen

  useScreenReady(!virtualKeys.isLoading);

  // `in-org` keeps the org-wide read apart from the per-scope one, whose key
  // is `["budgets", "org", <org id>]` when the picked scope is the org itself
  const budgets = useQuery({
    queryKey: orgRead ? ["budgets", "in-org", scope.orgId] : ["budgets", scopeType, scopeId],
    queryFn: () =>
      orgRead ? fetchOrgBudgets(scope.orgId as string) : fetchBudgets(scopeType, scopeId),
    enabled: orgRead ? !!scope.orgId : !!scopeId,
  });

  // the gateway's own counters, read through the control plane. a separate
  // query so a slow or failed read never holds the budgets themselves back
  const spend = useQuery({
    queryKey: orgRead
      ? ["budgets", "in-org", scope.orgId, "spend"]
      : ["budgets", scopeType, scopeId, "spend"],
    queryFn: () =>
      orgRead ? fetchOrgBudgetSpend(scope.orgId as string) : fetchBudgetSpend(scopeType, scopeId),
    enabled: orgRead ? !!scope.orgId : !!scopeId,
    refetchInterval: 30_000,
  });

  const rateLimits = useQuery({
    queryKey: orgRead
      ? ["rate-limits", "in-org", scope.orgId]
      : ["rate-limits", scopeType, scopeId],
    queryFn: () =>
      orgRead ? fetchOrgRateLimits(scope.orgId as string) : fetchRateLimits(scopeType, scopeId),
    enabled: orgRead ? !!scope.orgId : !!scopeId,
  });

  // a cap written on one scope changes the org-wide list too, so both go stale
  const invalidateBudgets = () => queryClient.invalidateQueries({ queryKey: ["budgets"] });
  const invalidateRateLimits = () => queryClient.invalidateQueries({ queryKey: ["rate-limits"] });

  // the org-wide read answers every scope type; the picked type narrows it
  const ofType = <T extends { scope_type: string }>(rows: T[] | undefined) =>
    rows && orgRead && scopeType !== ALL_SCOPES
      ? rows.filter(
          (row) =>
            row.scope_type === scopeType &&
            (!userScope || !scopeId || (row as { scope_id?: string }).scope_id === scopeId),
        )
      : rows;
  const budgetRows = ofType(budgets.data);
  const rateLimitRows = ofType(rateLimits.data);

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
      case USER_SCOPE:
        return (users.data ?? []).map((user) => ({
          value: user.id,
          label: personName(user),
          description: user.display_name ? user.email : undefined,
        }));
      default:
        return [];
    }
  })();

  // the scope field is a name picker whenever the scope type has rows to offer
  // and a bare uuid box otherwise; the hint has to say which one it is, since
  // "the project this cap applies to" reads as nonsense over an empty uuid
  // field (#1202)
  const hasPicker = scopeOptions.length > 0;
  // the read behind the picked type's picker. a read that has not answered holds
  // no rows either, so it must not read as an empty one (#2959)
  const pickerRead =
    scopeType === "virtual_key"
      ? { query: virtualKeys, target: "scope-virtual-keys", resource: "virtualKeys" }
      : scopeType === "business_unit"
        ? { query: businessUnits, target: "scope-business-units", resource: "businessUnits" }
        : scopeType === "customer"
          ? { query: customers, target: "scope-customers", resource: "customers" }
          : userScope
            ? { query: users, target: "scope-users", resource: "users" }
            : null;
  const pickerPending = !!pickerRead?.query.isLoading;
  // a scope typed in as a uuid has no name to show, and is shown as that uuid
  // a cap's scope by name. a key's name comes with the org-wide row itself
  // (`scope_name`, #3128); the start of its id is the fallback for a row with none
  const userName = (id: string) => {
    const user = users.data?.find((u) => u.id === id);
    return user ? personName(user) : undefined;
  };
  const nameOf = (type: string, id: string, rowName?: string | null): string => {
    if (type === "virtual_key" && rowName) return rowName;
    if (type === scopeType) {
      const picked = scopeOptions.find((option) => option.value === id)?.label;
      if (picked) return picked;
    }
    const named = (rows: { id: string; name: string }[] | undefined) =>
      rows?.find((row) => row.id === id)?.name;
    const found =
      type === "org"
        ? named(scope.orgs)
        : type === "team"
          ? named(orgScope.teams)
          : type === "project"
            ? named(orgScope.byTeam.flatMap((entry) => entry.projects))
            : type === "business_unit"
              ? named(businessUnits.data)
              : type === "customer"
                ? named(customers.data)
                : type === USER_SCOPE
                  ? userName(id)
                  : virtualKeys.data?.find((key) => key.id === id)?.name;
    return found || (type === "virtual_key" ? id.slice(0, 8) : id);
  };
  const scopeName = scopeId ? nameOf(scopeType, scopeId) : "";
  // a name is only unique within its type, so an org-wide row says which kind
  // it belongs to; the per-scope view already said it in the picker
  const scopeLabelOf = (
    row: { scope_type: string; scope_id: string; scope_name?: string | null } & UserScopeNarrowing,
  ) => {
    const type = orgWide ? row.scope_type : scopeType;
    return t("pages.limits.scopeNamed", {
      type: t(`pages.limits.scopeTypes.${type}`),
      // a user row names the person and what their cap counts, since the same
      // person can hold one cap per place it is narrowed to (#3282); until the
      // people are read it says only what it counts rather than showing a uuid
      name:
        type === USER_SCOPE
          ? userName(row.scope_id)
            ? t("pages.limits.userScopeNamed", {
                name: userName(row.scope_id),
                reach: t(`pages.limits.userScope.${capReach(row)}`),
              })
            : t(`pages.limits.userScope.${capReach(row)}`)
          : orgWide
            ? nameOf(row.scope_type, row.scope_id, row.scope_name)
            : scopeName,
    });
  };
  // the delete confirmations open on it, so the type comes with the name
  const auth = useOptionalAuth();
  // where a new user cap can be narrowed to, which is also who may manage it
  const userReach: UserReachOptions | undefined = userScope
    ? {
        orgs: scope.orgs.map((org) => ({ value: org.id, label: org.name })),
        projects: orgScope.byTeam.flatMap((entry) =>
          entry.projects.map((project) => ({ value: project.id, label: project.name })),
        ),
        orgId: scope.orgId ?? null,
        // an open-mode session has no account, and its caller acts as the admin
        // token, so it may leave a cap un-narrowed; the control plane decides
        canLeaveOpen: auth?.user ? auth.user.is_superadmin : true,
      }
    : undefined;
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
          <Field label={t("pages.limits.scopeTypeLabel")} info={t("pages.limits.scopeTypeInfo")}>
            <Combobox
              value={scopeType}
              onChange={(picked) => {
                setTypePicked(true);
                setScopeType(picked);
                setScopeId("");
              }}
              options={[ALL_SCOPES, ...SCOPE_TYPES].map((type) => ({
                value: type,
                label: t(`pages.limits.scopeTypes.${type}`),
              }))}
            />
          </Field>
          {scopeType !== ALL_SCOPES && (
            <Field
              label={t("pages.limits.scopeLabel")}
              hint={
                pickerPending
                  ? undefined
                  : t(
                      !hasPicker
                        ? "pages.limits.scopeIdHint"
                        : orgWide
                          ? "pages.limits.scopeHintList"
                          : "pages.limits.scopeHint",
                      { type: t(`pages.limits.scopeTypes.${scopeType}`) },
                    )
              }
            >
              {pickerPending ? (
                <ControlSkeleton width="100%" />
              ) : hasPicker ? (
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
          )}
          {scopeType === ALL_SCOPES && (
            <p className="text-xs text-muted-foreground sm:self-end">
              {t("pages.limits.scopeTypeHintAll")}
            </p>
          )}
          {/* the picker reads only feed the picker, which falls back to a bare uuid
              box without them, so a failed read is said here (#2642, #2959). the
              query keeps its error once another scope type is picked, hence
              the read is the picked type's own */}
          {pickerRead?.query.error && (
            <div className="sm:col-span-2">
              {/* load-error-allow: the scope picker's options; a picker has no empty state of its own */}
              <LoadError
                error={pickerRead.query.error}
                resource={t(`errors.resources.${pickerRead.resource}`)}
                onRetry={() => pickerRead.query.refetch()}
                target={pickerRead.target}
              />
            </div>
          )}
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
            target="budgets"
          />
        )}
        {budgetRows?.length === 0 && (
          <EmptyState
            thread
            uxTarget="budgets"
            icon={<Wallet />}
            title={t(
              !orgWide
                ? "pages.limits.budgetsEmptyTitle"
                : scopeType === ALL_SCOPES
                  ? "pages.limits.budgetsEmptyAllTitle"
                  : "pages.limits.budgetsEmptyTypeTitle",
            )}
            description={t(
              !orgWide
                ? "pages.limits.budgetsEmptyBody"
                : scopeType === ALL_SCOPES
                  ? "pages.limits.budgetsEmptyAllBody"
                  : "pages.limits.budgetsEmptyTypeBody",
              { type: t(`pages.limits.scopeTypes.${scopeType}`) },
            )}
            actions={
              // a cap hangs off one scope, so the org-wide view has nothing to add to
              orgWide ? undefined : (
                <GatedButton
                  gate="budget:create"
                  control="budget-new-empty"
                  onClick={() => setAddBudgetOpen(true)}
                >
                  {t("pages.limits.budgetsEmptyAction")}
                </GatedButton>
              )
            }
          />
        )}
        <ScopeGroups
          rows={budgetRows}
          grouped={orgWide}
          labelOf={scopeLabelOf}
          renderCard={(budget) => (
            <BudgetCard
              key={budget.id}
              budget={budget}
              scope={orgWide || userScope ? scopeLabelOf(budget) : scopeName}
              spend={spend.isLoading ? "loading" : spend.isError ? null : (spend.data ?? null)}
              gateAt={capGateScope(budget, {
                byTeam: orgScope.byTeam,
                keyProjectId: orgWide ? undefined : scope.projectId,
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
          )}
        />
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
            target="rate-limits"
          />
        )}
        {rateLimitRows?.length === 0 && (
          <EmptyState
            thread
            uxTarget="rate-limits"
            icon={<Gauge />}
            title={t(
              !orgWide
                ? "pages.limits.rateLimitsEmptyTitle"
                : scopeType === ALL_SCOPES
                  ? "pages.limits.rateLimitsEmptyAllTitle"
                  : "pages.limits.rateLimitsEmptyTypeTitle",
            )}
            description={t(
              !orgWide
                ? "pages.limits.rateLimitsEmptyBody"
                : scopeType === ALL_SCOPES
                  ? "pages.limits.rateLimitsEmptyAllBody"
                  : "pages.limits.rateLimitsEmptyTypeBody",
              { type: t(`pages.limits.scopeTypes.${scopeType}`) },
            )}
            actions={
              orgWide ? undefined : (
                <GatedButton
                  gate="rate_limit:create"
                  control="rate-limit-new-empty"
                  onClick={() => setAddRateLimitOpen(true)}
                >
                  {t("pages.limits.rateLimitsEmptyAction")}
                </GatedButton>
              )
            }
          />
        )}
        <ScopeGroups
          rows={rateLimitRows}
          grouped={orgWide}
          labelOf={scopeLabelOf}
          renderCard={(limit) => (
            <RateLimitCard
              key={limit.id}
              limit={limit}
              scope={orgWide || userScope ? scopeLabelOf(limit) : scopeName}
              gateAt={capGateScope(limit, {
                byTeam: orgScope.byTeam,
                keyProjectId: orgWide ? undefined : scope.projectId,
              })}
              onEdit={() => {
                setEditingRateLimit(limit);
                setEditRateLimitOpen(true);
              }}
              onDelete={() => setRateLimitToDelete(limit)}
              deleting={removeRateLimit.isPending && rateLimitToDelete?.id === limit.id}
            />
          )}
        />
      </div>

      <BudgetSheet
        open={addBudgetOpen}
        onOpenChange={setAddBudgetOpen}
        scopeType={scopeType}
        scopeId={scopeId}
        scopeName={scopeName}
        userReach={userReach}
        onDone={invalidateBudgets}
      />
      {editingBudget && (
        <BudgetSheet
          open={editBudgetOpen}
          onOpenChange={setEditBudgetOpen}
          scopeType={editingBudget.scope_type}
          scopeId={editingBudget.scope_id}
          scopeName={nameOf(
            editingBudget.scope_type,
            editingBudget.scope_id,
            editingBudget.scope_name,
          )}
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
        userReach={userReach}
        onDone={invalidateRateLimits}
      />
      {editingRateLimit && (
        <RateLimitSheet
          open={editRateLimitOpen}
          onOpenChange={setEditRateLimitOpen}
          scopeType={editingRateLimit.scope_type}
          scopeId={editingRateLimit.scope_id}
          scopeName={nameOf(
            editingRateLimit.scope_type,
            editingRateLimit.scope_id,
            editingRateLimit.scope_name,
          )}
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
          budgetToDelete
            ? t("pages.limits.confirm.budgetBody", { scope: scopeLabelOf(budgetToDelete) })
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
            ? t("pages.limits.confirm.rateLimitBody", { scope: scopeLabelOf(rateLimitToDelete) })
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

/**
 * The cards of one list. Per scope they are one grid; org-wide they are grouped
 * under the scope each belongs to, scope types in the order the picker offers
 * them and scopes by name, so "which caps exist here" reads scope by scope
 * (#2093).
 */
function ScopeGroups<
  T extends { id: string; scope_type: string; scope_id: string } & UserScopeNarrowing,
>({
  rows,
  grouped,
  labelOf,
  renderCard,
}: {
  rows: T[] | undefined;
  grouped: boolean;
  labelOf: (row: T) => string;
  renderCard: (row: T) => React.ReactNode;
}) {
  const grid = "grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(280px,100%),1fr))]";
  if (!rows?.length) return null;
  if (!grouped) return <div className={grid}>{rows.map(renderCard)}</div>;
  const groups = new Map<string, T[]>();
  for (const row of rows) {
    // one person's caps narrowed differently are different groups
    const key = `${row.scope_type}:${row.scope_id}:${row.within_org_id ?? ""}:${row.within_project_id ?? ""}`;
    groups.set(key, [...(groups.get(key) ?? []), row]);
  }
  // a type the picker does not list (a user) sorts after the ones it does
  const typeOrder = (row: T) => {
    const at = SCOPE_TYPES.indexOf(row.scope_type as (typeof SCOPE_TYPES)[number]);
    return at < 0 ? SCOPE_TYPES.length : at;
  };
  const ordered = [...groups.entries()]
    .map(([key, members]) => ({ key, members, label: labelOf(members[0]) }))
    .sort(
      (a, b) => typeOrder(a.members[0]) - typeOrder(b.members[0]) || a.label.localeCompare(b.label),
    );
  return (
    <div className="space-y-4">
      {ordered.map((group) => (
        <div key={group.key} className="space-y-2">
          <h3 className="text-sm font-medium text-muted-foreground">{group.label}</h3>
          <div className={grid}>{group.members.map(renderCard)}</div>
        </div>
      ))}
    </div>
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
 * read it the card says so; a missing figure is never drawn as an empty bar,
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

function BudgetCard({
  budget,
  scope,
  spend,
  onEdit,
  onDelete,
  deleting,
  gateAt,
}: {
  budget: BudgetRow;
  scope: string;
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
      <BudgetSpend budget={budget} spend={spend} />
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
  userReach,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  scopeType: string;
  scopeId: string;
  /** what the subtitle calls the scope: its name, not the `type:uuid` it is stored as */
  scopeName: string;
  budget?: BudgetRow;
  /** a new user cap picks the place it is narrowed to (#3282) */
  userReach?: UserReachOptions;
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
  const [reach, setReach] = React.useState<ReachDraft>(() => initialReach(userReach));

  React.useEffect(() => {
    if (open) {
      setLimitUsd(seed.limitUsd);
      setPeriod(seed.period);
      setUnpriced(seed.unpriced);
      setReach(initialReach(userReach));
    }
    // the options are rebuilt every render; the sheet resets when it opens
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
        ...(userReach ? withinOf(reach) : {}),
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
      subtitle={t("pages.limits.budgetSheetSubtitle", { scope: scopeName })}
      dirty={dirty}
      errorMessage={save.isError ? errorText(save.error, t) : undefined}
      saveLabel={t(budget ? "common.save" : "common.create")}
      canSave={
        Boolean(limitUsd.trim() && periodKind(period)) &&
        (!budget || dirty) &&
        (!userReach || reachComplete(reach))
      }
      saving={save.isPending}
      onSubmit={() => save.mutate()}
    >
      <div className="space-y-3">
        {userReach && <UserReachFields value={reach} onChange={setReach} options={userReach} />}
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
  scopeType,
  scopeId,
  scopeName,
  limit,
  userReach,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  scopeType: string;
  scopeId: string;
  /** what the subtitle calls the scope: its name, not the `type:uuid` it is stored as */
  scopeName: string;
  limit?: RateLimitRow;
  /** a new user cap picks the place it is narrowed to (#3282) */
  userReach?: UserReachOptions;
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
  const [reach, setReach] = React.useState<ReachDraft>(() => initialReach(userReach));

  React.useEffect(() => {
    if (open) {
      setRpm(seed.rpm);
      setTpm(seed.tpm);
      setReach(initialReach(userReach));
    }
    // the options are rebuilt every render; the sheet resets when it opens
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
        ...(userReach ? withinOf(reach) : {}),
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
      subtitle={t("pages.limits.rateLimitSheetSubtitle", { scope: scopeName })}
      dirty={dirty}
      errorMessage={save.isError ? errorText(save.error, t) : undefined}
      saveLabel={t(limit ? "common.save" : "common.create")}
      canSave={
        Boolean(rpm.trim() || tpm.trim()) &&
        (!limit || dirty) &&
        (!userReach || reachComplete(reach))
      }
      // either cap will do, so neither field is `required` on its own: a refused
      // submit with both empty says so in the footer instead
      blockedMessage={rpm.trim() || tpm.trim() ? undefined : t("pages.limits.rateLimitNeedsOne")}
      saving={save.isPending}
      onSubmit={() => save.mutate()}
    >
      <div className="space-y-3">
        {userReach && <UserReachFields value={reach} onChange={setReach} options={userReach} />}
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
