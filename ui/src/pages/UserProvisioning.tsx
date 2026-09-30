import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { AlertTriangle, BookUser, Loader2, Plus, Trash2, Users } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { LoadingRegion, TableSkeleton } from "@/components/LoadingState";
import {
  OrgScopePicker,
  OrgScopePill,
  scopeTargetIds,
  useOrgScope,
  type ScopeTarget,
} from "@/components/OrgScopePicker";
import { CopyButton } from "@/components/CopyButton";
import { ListSummary, PageBody, RowIconButton } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { FieldLabel } from "@/components/ui/field-label";
import { Input } from "@/components/ui/input";
import { Sheet, SheetActions, SheetBody, SheetFooter, SheetHeader } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, type TableColumn } from "@/components/ui/table";
import {
  createScimGroupMapping,
  createScimToken,
  deleteScimGroupMapping,
  fetchScimGroupMappings,
  fetchScimTokens,
  revokeScimToken,
  scimBaseUrl,
  ApiError,
  ROLES,
  type CreatedScimToken,
  type ScimGroupMappingRow,
  type ScimTokenRow,
} from "@/lib/api";
import { useFormat, type Formatters } from "@/lib/i18n/format";
import { useScope } from "@/lib/scope";
import { useToast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { usePublicUrl } from "@/lib/use-public-url";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

const TOKENS_QUERY_KEY = ["scim-tokens"];
const MAPPINGS_QUERY_KEY = "scim-group-mappings";

// the roles a mapping may grant, mirroring `parse_role` in
// crates/rolter-control/src/sso.rs, which scim_groups.rs calls verbatim. not
// /api/v1/roles: that list carries every role the control plane knows about,
// and offering one the endpoint refuses would build a form that can only fail
// on submit
const MAPPABLE_ROLES = ROLES;

// listing, creating and revoking all require Admin on the org. a 403 is a
// permission answer, not a failure, so it gets its own calm state rather than
// an error banner
function isForbidden(error: unknown): boolean {
  return error instanceof ApiError && error.status === 403;
}

function stamp(fmt: Formatters, iso: string | null): string {
  return (iso ? fmt.dateTime(iso) : "") || "—";
}

// a moment in a table cell. every date column shows the day and keeps the
// whole stamp for the hover, so "last sync" and "created" read the same way
// and the clock is still one hover away
function When({ fmt, iso }: { fmt: Formatters; iso: string }) {
  const day = fmt.date(iso);
  if (!day) return <>—</>;
  return (
    <time dateTime={iso} title={stamp(fmt, iso)}>
      {day}
    </time>
  );
}

// the label for a role the server sent us, falling back to the raw value so a
// newer control plane's role is shown rather than rendered as a missing key
function roleLabel(t: TFunction, role: string): string {
  return t(`shell.roles.${role}`, { defaultValue: role });
}

// a value to copy out of the dashboard and into the identity provider's
// connector. mono and wrapping rather than truncated, because an address or a
// token is checked by its end, and the copy button is named for what it copies
function CopyBox({
  value,
  copyLabel,
  testId,
  className,
}: {
  value: string;
  copyLabel: string;
  testId: string;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex items-center justify-between gap-2 rounded-md border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] py-1.5 pl-3 pr-1.5",
        className,
      )}
    >
      <code data-testid={testId} className="min-w-0 break-all font-mono text-sm text-foreground">
        {value}
      </code>
      <CopyButton value={value} label={copyLabel} />
    </div>
  );
}

/**
 * The address an identity provider's SCIM connector is pointed at (#2079).
 *
 * The control plane builds every address it gives an outside caller from
 * `ROLTER_PUBLIC_URL`, never from the request, and the dashboard may be open
 * under a different name than the one the provider must call. So the base is
 * read from the control plane (the query the Single Sign-On screen shares) and
 * `/scim/v2` is appended to it, never to `window.location`. A wrong value
 * fails in the provider's own test console with no hint from rolter, and the
 * reveal step is the one window where an operator is also holding a secret
 * that will not be shown again, so the address sits beside the token.
 *
 * Unset, the base is the control plane's default, which only a caller on its own
 * host can reach; the value is still shown and copyable, with that said under
 * it rather than left for the provider's error to say later. The read does not
 * gate the screen: pending holds the space, and a failed read says so with a
 * retry instead of a URL that might be wrong.
 */
function ScimBaseUrl({ hint, className }: { hint?: string; className?: string }) {
  const { t } = useTranslation();
  const publicUrl = usePublicUrl();
  const labelId = React.useId();
  const value = publicUrl.data ? scimBaseUrl(publicUrl.data.public_url) : null;
  return (
    <div role="group" aria-labelledby={labelId} className="flex min-w-0 flex-col gap-1.5">
      <FieldLabel id={labelId} label={t("pages.userProvisioning.baseUrl.label")} />
      {publicUrl.isError ? (
        <LoadError
          error={publicUrl.error}
          resource={t("errors.resources.publicUrl")}
          onRetry={() => publicUrl.refetch()}
        />
      ) : value ? (
        <CopyBox
          value={value}
          copyLabel={t("pages.userProvisioning.baseUrl.copy")}
          testId="scim-base-url"
          className={className}
        />
      ) : (
        <LoadingRegion className={cn("w-full", className)}>
          <Skeleton height={46} radius={6} />
        </LoadingRegion>
      )}
      {hint && value && <p className="text-xs text-muted-foreground">{hint}</p>}
      {publicUrl.data?.configured === false && (
        <p
          role="note"
          className="flex items-start gap-1.5 text-xs text-[color:var(--status-warning-text)]"
        >
          <AlertTriangle aria-hidden className="mt-px h-3.5 w-3.5 flex-none" />
          <span>
            <Trans
              i18nKey="pages.userProvisioning.baseUrl.default"
              values={{ url: publicUrl.data.public_url }}
              components={{ code: <code className="font-mono" /> }}
            />
          </span>
        </p>
      )}
    </div>
  );
}

/**
 * The IdP groups this org turns into roles (#1186).
 *
 * A group the IdP pushes through `/scim/v2/Groups` grants nothing on its own —
 * a provisioned account joins as a viewer and stops there. This is where an
 * operator says what a group is worth, and the control plane reconciles
 * everyone in it on the spot rather than at the next sync.
 *
 * The scope select is the shared `OrgScopePicker`: the org, every team in it,
 * and every project in any of those teams, so a mapping onto a project in a
 * team the scope switcher does not currently have selected can be written
 * without moving the switcher first (#1249). A mapping may never grant outside
 * its own org anyway.
 */
function GroupMappings({ orgId, canManage }: { orgId: string; canManage: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const scope = useOrgScope(orgId);
  const toast = useToast();

  const mappings = useQuery({
    queryKey: [MAPPINGS_QUERY_KEY, orgId],
    queryFn: () => fetchScimGroupMappings(orgId),
    retry: false,
  });

  const rows = mappings.data ?? [];

  const invalidate = () => queryClient.invalidateQueries({ queryKey: [MAPPINGS_QUERY_KEY, orgId] });

  const [group, setGroup] = React.useState("");
  const [role, setRole] = React.useState<string>(MAPPABLE_ROLES[0]);
  // "" is the org; otherwise "team:<id>" or "project:<id>"
  const [target, setTarget] = React.useState<ScopeTarget>("");

  const create = useMutation({
    mutationFn: () => {
      return createScimGroupMapping(orgId, {
        group_name: group.trim(),
        role,
        ...scopeTargetIds(target),
      });
    },
    // the failure stays inline, beside the form that caused it; the success is
    // what would otherwise be silent, so that is the one that toasts (#1197)
    onSuccess: (created) => {
      setGroup("");
      invalidate();
      toast.push({
        tone: "success",
        title: t("toast.created", { what: created.group_name }),
      });
    },
  });

  const remove = useMutation({
    mutationFn: (id: string) => deleteScimGroupMapping(id),
    onSuccess: (_void, id) => {
      invalidate();
      toast.push({
        tone: "success",
        title: t("toast.deleted", {
          what: rows.find((m) => m.id === id)?.group_name ?? id,
        }),
      });
    },
  });

  // a mapping is what puts people in a role, so removing one takes access away
  // from everyone in that group — named and confirmed first (#1179)
  const [removeTarget, setRemoveTarget] = React.useState<ScimGroupMappingRow | null>(null);
  const startRemove = (mapping: ScimGroupMappingRow) => {
    remove.reset();
    setRemoveTarget(mapping);
  };

  return (
    <section className="rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-card)]">
      <header className="flex items-start gap-3 border-b border-[color:var(--border-subtle)] px-4 py-3">
        <Users aria-hidden className="mt-0.5 h-4 w-4 flex-none text-muted-foreground" />
        <div className="min-w-0">
          <h2 className="text-sm font-medium text-foreground">
            {t("pages.userProvisioning.mappings.title")}
          </h2>
          <p className="mt-1 text-sm text-muted-foreground">
            {t("pages.userProvisioning.mappings.subtitle")}
          </p>
        </div>
      </header>

      <div className="flex flex-col gap-2.5 px-4 py-3.5">
        {mappings.isLoading && <Skeleton className="h-8 rounded-md" />}
        {mappings.isError && (
          <LoadError
            error={mappings.error}
            resource={t("errors.resources.scimGroupMappings")}
            onRetry={() => mappings.refetch()}
          />
        )}
        {!mappings.isLoading && !mappings.isError && rows.length === 0 && (
          <p className="text-sm text-muted-foreground">
            {t("pages.userProvisioning.mappings.empty")}
          </p>
        )}

        {rows.length > 0 && (
          <ul className="flex flex-col gap-1.5">
            {rows.map((mapping) => (
              <li
                key={mapping.id}
                className="flex items-center gap-2 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-2.5 py-1.5"
              >
                <span className="min-w-0 flex-1 truncate font-mono text-xs text-foreground">
                  {mapping.group_name}
                </span>
                <OrgScopePill scope={scope} value={mapping} />
                <Badge tone="neutral">{roleLabel(t, mapping.role)}</Badge>
                <RowIconButton
                  danger
                  gate="scim_group_mapping:delete"
                  control="scim-mapping-remove"
                  title={t("pages.userProvisioning.mappings.remove")}
                  aria-label={t("pages.userProvisioning.mappings.removeNamed", {
                    group: mapping.group_name,
                  })}
                  disabled={remove.isPending}
                  onClick={() => startRemove(mapping)}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </RowIconButton>
              </li>
            ))}
          </ul>
        )}

        <div className="flex flex-wrap items-center gap-2">
          <Input
            className="h-8 max-w-[220px] flex-1"
            value={group}
            onChange={(e) => setGroup(e.target.value)}
            aria-label={t("pages.userProvisioning.mappings.groupLabel")}
            placeholder={t("pages.userProvisioning.mappings.groupPlaceholder")}
          />
          <OrgScopePicker
            orgId={orgId}
            value={target}
            onChange={setTarget}
            label={t("pages.userProvisioning.mappings.scopeLabel")}
          />
          <Combobox
            size="sm"
            className="w-[132px]"
            value={role}
            onChange={setRole}
            aria-label={t("pages.userProvisioning.mappings.roleLabel")}
            options={MAPPABLE_ROLES.map((r) => ({ value: r, label: roleLabel(t, r) }))}
          />
          <GatedButton
            gate="scim_group_mapping:create"
            control="scim-mapping-add"
            size="sm"
            variant="outline"
            disabled={!canManage || !group.trim() || create.isPending}
            onClick={() => create.mutate()}
          >
            {create.isPending && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
            {t("pages.userProvisioning.mappings.add")}
          </GatedButton>
        </div>
        {/* the control plane's own message, never a gloss on it */}
        {create.isError && (
          <p role="alert" className="text-sm text-[color:var(--status-danger-text)]">
            {(create.error as Error).message}
          </p>
        )}
      </div>

      <ConfirmDialog
        name="scim-group-mapping-remove"
        open={!!removeTarget}
        onOpenChange={(open) => !open && setRemoveTarget(null)}
        title={t("pages.userProvisioning.mappings.confirm.title", {
          group: removeTarget?.group_name,
        })}
        description={t("pages.userProvisioning.mappings.confirm.body", {
          role: removeTarget ? roleLabel(t, removeTarget.role) : "",
        })}
        confirmLabel={t("pages.userProvisioning.mappings.confirm.confirm")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() =>
          removeTarget &&
          remove.mutate(removeTarget.id, {
            onSuccess: () => setRemoveTarget(null),
          })
        }
      />
    </section>
  );
}

// SCIM provisioning tokens for /api/v1/orgs/{org}/scim-tokens (#540, #563).
// the screen manages the credentials an IdP authenticates with — the SCIM
// resource endpoints under /scim/v2 are driven by the IdP, never from here
export default function UserProvisioning() {
  const { t } = useTranslation();
  const toast = useToast();
  const fmt = useFormat();
  const queryClient = useQueryClient();
  const scope = useScope();
  const orgId = scope.orgId;

  const tokens = useQuery({
    queryKey: [...TOKENS_QUERY_KEY, orgId],
    queryFn: () => fetchScimTokens(orgId as string),
    enabled: !!orgId,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;

  // `tokens` is the query the user is actually waiting on for this screen

  useScreenReady(!tokens.isLoading);

  useErrorState(!!tokens.error, "user-provisioning");

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: [...TOKENS_QUERY_KEY, orgId] });

  const revoke = useMutation({
    mutationFn: (id: string) => revokeScimToken(id),
    onSuccess: invalidate,
  });

  const [issueOpen, setIssueOpen] = React.useState(false);
  const [revokeTarget, setRevokeTarget] = React.useState<ScimTokenRow | null>(null);

  const forbidden = isForbidden(tokens.error);
  // no org means no place to hang a token; a 403 means this principal may look
  // at nothing here, so both disable the mint button instead of failing later
  const canManage = !!orgId && !forbidden;

  const columns: TableColumn<ScimTokenRow & Record<string, unknown>>[] = [
    { key: "name", header: t("pages.userProvisioning.columns.token") },
    {
      key: "revoked_at",
      header: t("pages.userProvisioning.columns.status"),
      render: (_v, row) =>
        row.revoked_at ? (
          <Badge
            tone="danger"
            title={t("pages.userProvisioning.revokedAt", {
              when: stamp(fmt, row.revoked_at),
            })}
          >
            {t("pages.userProvisioning.statusRevoked")}
          </Badge>
        ) : (
          <Badge dot tone="success">
            {t("pages.userProvisioning.statusActive")}
          </Badge>
        ),
    },
    {
      key: "last_used_at",
      header: t("pages.userProvisioning.columns.lastSync"),
      render: (_v, row) =>
        row.last_used_at ? (
          <When fmt={fmt} iso={row.last_used_at} />
        ) : (
          <span className="text-[color:var(--text-subtle)]">
            {t("pages.userProvisioning.neverUsed")}
          </span>
        ),
    },
    {
      key: "created_at",
      header: t("pages.userProvisioning.columns.created"),
      align: "right",
      render: (_v, row) => <When fmt={fmt} iso={row.created_at} />,
    },
    {
      key: "actions",
      // an empty <th> leaves the cells under it unnamed; the column is real,
      // it just has nothing worth drawing (#1244)
      header: <span className="sr-only">{t("common.rowActions")}</span>,
      align: "right",
      render: (_v, row) => (
        <GatedButton
          gate="scim_token:delete"
          control="scim-token-revoke"
          variant="outline"
          size="sm"
          aria-label={t("pages.userProvisioning.revokeAria", { name: row.name })}
          disabled={!!row.revoked_at || revoke.isPending}
          onClick={() => {
            revoke.reset();
            setRevokeTarget(row);
          }}
        >
          {row.revoked_at
            ? t("pages.userProvisioning.revoked")
            : t("pages.userProvisioning.revoke")}
        </GatedButton>
      ),
    },
  ];

  if (scope.isLoading || (tokens.isLoading && !!orgId)) {
    return (
      <PageBody>
        <TableSkeleton rows={4} />
      </PageBody>
    );
  }

  const rows = tokens.data ?? [];
  const active = rows.filter((t) => !t.revoked_at).length;

  return (
    <PageBody>
      <div className="flex flex-wrap items-center gap-3">
        {/* the explanation stays when the list is unread; the count does not,
            since "0 tokens" is not what a failed read found (#2211) */}
        <ListSummary
          data={tokens.data}
          fallback={
            <Trans
              i18nKey="pages.userProvisioning.leadUnread"
              components={[<code key="path" className="font-mono text-xs" />]}
            />
          }
        >
          {(all) => (
            <Trans
              i18nKey="pages.userProvisioning.lead"
              count={all.length}
              values={{ active }}
              components={[<code key="path" className="font-mono text-xs" />]}
            />
          )}
        </ListSummary>
        <div className="ml-auto">
          <GatedButton
            gate="scim_token:create"
            control="scim-token-new"
            disabled={!canManage}
            onClick={() => setIssueOpen(true)}
          >
            <Plus className="h-4 w-4" />
            {t("pages.userProvisioning.issueToken")}
          </GatedButton>
        </div>
      </div>

      {/* only beside a token list that was read: a caller refused it, or a
          control plane with no store, has no connector to point anywhere */}
      {orgId && tokens.isSuccess && (
        <ScimBaseUrl className="max-w-xl" hint={t("pages.userProvisioning.baseUrl.hint")} />
      )}

      {forbidden && (
        <p className="text-sm text-muted-foreground">{t("pages.userProvisioning.forbidden")}</p>
      )}
      {tokens.isError && !forbidden && (
        <LoadError
          error={tokens.error}
          resource={t("errors.resources.provisioningTokens")}
          onRetry={() => tokens.refetch()}
        />
      )}
      {revoke.isError && (
        <p className="text-sm text-[color:var(--status-danger-text)]">
          {(revoke.error as Error).message}
        </p>
      )}

      {!forbidden && (
        <Table
          columns={columns}
          data={rows as (ScimTokenRow & Record<string, unknown>)[]}
          rowKey="id"
          read={tokens}
          empty={
            <EmptyState
              uxTarget="provisioning-list"
              icon={<BookUser />}
              title={t("pages.userProvisioning.emptyTitle")}
              description={t("pages.userProvisioning.emptyBody")}
              actions={
                <GatedButton
                  gate="scim_token:create"
                  control="scim-token-new-empty"
                  disabled={!canManage}
                  onClick={() => setIssueOpen(true)}
                >
                  {t("pages.userProvisioning.emptyAction")}
                </GatedButton>
              }
            />
          }
        />
      )}

      {/* the second half of provisioning: who exists comes from /scim/v2/Users,
          what they may do comes from a mapping written here (#1186) */}
      {orgId && !forbidden && <GroupMappings orgId={orgId} canManage={canManage} />}

      {orgId && (
        <IssueTokenSheet
          open={issueOpen}
          onOpenChange={setIssueOpen}
          orgId={orgId}
          onIssued={invalidate}
        />
      )}

      <ConfirmDialog
        name="scim-token-revoke"
        open={!!revokeTarget}
        onOpenChange={(open) => !open && setRevokeTarget(null)}
        title={t("pages.userProvisioning.revokeTitle")}
        description={
          <Trans
            i18nKey="pages.userProvisioning.revokeBody"
            values={{ name: revokeTarget?.name }}
            components={[<span key="name" className="font-mono" />]}
          />
        }
        confirmLabel={t("pages.userProvisioning.revoke")}
        pending={revoke.isPending}
        error={revoke.error}
        onConfirm={() => {
          if (!revokeTarget) return;
          const what = revokeTarget.name;
          revoke.mutate(revokeTarget.id, {
            onSuccess: () => {
              setRevokeTarget(null);
              toast.push({ tone: "success", title: t("toast.revoked", { what }) });
            },
          });
        }}
      />
    </PageBody>
  );
}

// mint a token, then hand over the plaintext. the secret lives in this
// component's state only and is dropped on close — the server stores a peppered
// digest, so nothing can show it a second time
function IssueTokenSheet({
  open,
  onOpenChange,
  orgId,
  onIssued,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  onIssued: () => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = React.useState("");
  const [issued, setIssued] = React.useState<CreatedScimToken | null>(null);
  const tokenLabelId = React.useId();

  React.useEffect(() => {
    if (open) {
      setName("");
      setIssued(null);
    }
  }, [open]);

  const create = useMutation({
    mutationFn: () => createScimToken(orgId, { name: name.trim() }),
    onSuccess: (token) => {
      setIssued(token);
      onIssued();
    },
  });

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetHeader
        title={
          issued
            ? t("pages.userProvisioning.sheet.copyTitle")
            : t("pages.userProvisioning.sheet.issueTitle")
        }
        subtitle={issued ? issued.name : t("pages.userProvisioning.sheet.subtitle")}
        onClose={() => onOpenChange(false)}
      />
      <SheetBody>
        {issued ? (
          <>
            <div
              role="group"
              aria-labelledby={tokenLabelId}
              className="flex min-w-0 flex-col gap-1.5"
            >
              <FieldLabel id={tokenLabelId} label={t("pages.userProvisioning.sheet.tokenLabel")} />
              <CopyBox
                value={issued.secret}
                copyLabel={t("pages.userProvisioning.copyToken")}
                testId="scim-token-secret"
              />
            </div>
            <p className="text-sm font-medium text-[color:var(--status-warning-text)]">
              {t("pages.userProvisioning.onceWarning")}
            </p>
            <ScimBaseUrl />
            <p className="text-sm text-muted-foreground">{t("pages.userProvisioning.pasteHint")}</p>
          </>
        ) : (
          <>
            <Field
              label={t("pages.userProvisioning.nameLabel")}
              hint={t("pages.userProvisioning.nameHint")}
            >
              <Input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={t("pages.userProvisioning.namePlaceholder")}
              />
            </Field>
            <p className="text-sm text-muted-foreground">
              {t("pages.userProvisioning.viewerNote")}
            </p>
            {create.isError && (
              <p className="text-sm text-[color:var(--status-danger-text)]">
                {(create.error as Error).message}
              </p>
            )}
          </>
        )}
      </SheetBody>
      <SheetFooter>
        <SheetActions>
          {issued ? (
            <Button onClick={() => onOpenChange(false)}>{t("common.done")}</Button>
          ) : (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                {t("common.cancel")}
              </Button>
              <Button disabled={!name.trim() || create.isPending} onClick={() => create.mutate()}>
                {create.isPending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
                {t("pages.userProvisioning.issueToken")}
              </Button>
            </>
          )}
        </SheetActions>
      </SheetFooter>
    </Sheet>
  );
}
