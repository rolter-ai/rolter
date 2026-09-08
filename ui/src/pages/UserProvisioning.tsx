import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { BookUser, Loader2, Plus, Trash2, Users } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { TableSkeleton } from "@/components/LoadingState";
import { CopyButton } from "@/components/CopyButton";
import { PageBody, Pill, RowIconButton } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Sheet, SheetBody, SheetFooter, SheetHeader } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, type TableColumn } from "@/components/ui/table";
import {
  createScimGroupMapping,
  createScimToken,
  deleteScimGroupMapping,
  fetchScimGroupMappings,
  fetchScimTokens,
  revokeScimToken,
  ApiError,
  ROLES,
  type CreatedScimToken,
  type ScimGroupMappingRow,
  type ScimTokenRow,
} from "@/lib/api";
import { useFormat, type Formatters } from "@/lib/i18n/format";
import { useScope, type ScopeResult } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
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

// the label for a role the server sent us, falling back to the raw value so a
// newer control plane's role is shown rather than rendered as a missing key
function roleLabel(t: TFunction, role: string): string {
  return t(`shell.roles.${role}`, { defaultValue: role });
}

// the scope a mapping grants at, as the reader knows it. the most specific
// non-null id wins, exactly as `scim_groups.rs` resolves it; an id the current
// scope selection does not cover is shown raw rather than hidden
function scopeLabel(
  t: TFunction,
  scope: ScopeResult,
  mapping: ScimGroupMappingRow,
): string {
  if (mapping.project_id) {
    return (
      scope.projects.find((p) => p.id === mapping.project_id)?.name ??
      mapping.project_id
    );
  }
  if (mapping.team_id) {
    return scope.teams.find((x) => x.id === mapping.team_id)?.name ?? mapping.team_id;
  }
  return t("pages.userProvisioning.mappings.scopeOrg");
}

/**
 * The IdP groups this org turns into roles (#1186).
 *
 * A group the IdP pushes through `/scim/v2/Groups` grants nothing on its own —
 * a provisioned account joins as a viewer and stops there. This is where an
 * operator says what a group is worth, and the control plane reconciles
 * everyone in it on the spot rather than at the next sync.
 *
 * The scope select offers the org, every team in it, and the projects of the
 * team the scope switcher currently has selected: those are the ids
 * `useScope()` has names for, and a mapping may never grant outside its own org
 * anyway.
 */
function GroupMappings({ orgId, canManage }: { orgId: string; canManage: boolean }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const scope = useScope();
  const toast = useToast();

  const mappings = useQuery({
    queryKey: [MAPPINGS_QUERY_KEY, orgId],
    queryFn: () => fetchScimGroupMappings(orgId),
    retry: false,
  });

  const rows = mappings.data ?? [];

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: [MAPPINGS_QUERY_KEY, orgId] });

  const [group, setGroup] = React.useState("");
  const [role, setRole] = React.useState<string>(MAPPABLE_ROLES[0]);
  // "" is the org; otherwise "team:<id>" or "project:<id>"
  const [target, setTarget] = React.useState("");

  const create = useMutation({
    mutationFn: () => {
      const [kind, id] = target.split(":");
      return createScimGroupMapping(orgId, {
        group_name: group.trim(),
        role,
        team_id: kind === "team" ? id : undefined,
        project_id: kind === "project" ? id : undefined,
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
  const [removeTarget, setRemoveTarget] =
    React.useState<ScimGroupMappingRow | null>(null);
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
                <Pill color="var(--text-secondary)" tint="var(--surface-card)">
                  {scopeLabel(t, scope, mapping)}
                </Pill>
                <Badge tone="neutral">{roleLabel(t, mapping.role)}</Badge>
                <RowIconButton
                  danger
                  gate="scim_group_mapping:delete"
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
          <Select
            className="h-8 w-[164px]"
            value={target}
            onChange={(e) => setTarget(e.target.value)}
            aria-label={t("pages.userProvisioning.mappings.scopeLabel")}
          >
            <option value="">{t("pages.userProvisioning.mappings.scopeOrg")}</option>
            {scope.teams.length > 0 && (
              <optgroup label={t("pages.userProvisioning.mappings.scopeTeams")}>
                {scope.teams.map((team) => (
                  <option key={team.id} value={`team:${team.id}`}>
                    {team.name}
                  </option>
                ))}
              </optgroup>
            )}
            {scope.projects.length > 0 && (
              <optgroup label={t("pages.userProvisioning.mappings.scopeProjects")}>
                {scope.projects.map((project) => (
                  <option key={project.id} value={`project:${project.id}`}>
                    {project.name}
                  </option>
                ))}
              </optgroup>
            )}
          </Select>
          <Select
            className="h-8 w-[132px]"
            value={role}
            onChange={(e) => setRole(e.target.value)}
            aria-label={t("pages.userProvisioning.mappings.roleLabel")}
          >
            {MAPPABLE_ROLES.map((r) => (
              <option key={r} value={r}>
                {roleLabel(t, r)}
              </option>
            ))}
          </Select>
          <GatedButton
            gate="scim_group_mapping:create"
            size="sm"
            variant="outline"
            disabled={!canManage || !group.trim() || create.isPending}
            onClick={() => create.mutate()}
          >
            {create.isPending && (
              <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
            )}
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
    { key: "name", header: "Token" },
    {
      key: "revoked_at",
      header: "Status",
      render: (_v, row) =>
        row.revoked_at ? (
          <Badge tone="danger" title={`revoked ${stamp(fmt, row.revoked_at)}`}>
            REVOKED
          </Badge>
        ) : (
          <Badge dot tone="success">
            ACTIVE
          </Badge>
        ),
    },
    {
      key: "last_used_at",
      header: "Last sync",
      render: (_v, row) =>
        row.last_used_at ? (
          stamp(fmt, row.last_used_at)
        ) : (
          <span className="text-[color:var(--text-subtle)]">never used</span>
        ),
    },
    {
      key: "created_at",
      header: "Created",
      align: "right",
      // "created" is a day, not an instant — the short date, as everywhere else
      render: (_v, row) => fmt.date(row.created_at) || "—",
    },
    {
      key: "actions",
      header: "",
      align: "right",
      render: (_v, row) => (
        <GatedButton
          gate="scim_token:delete"
          variant="outline"
          size="sm"
          aria-label={t("pages.userProvisioning.revokeAria", { name: row.name })}
          disabled={!!row.revoked_at || revoke.isPending}
          onClick={() => setRevokeTarget(row)}
        >
          {row.revoked_at ? "Revoked" : "Revoke"}
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
        <span className="text-sm text-muted-foreground">
          {rows.length} tokens · {active} active. Your identity provider presents
          one of these as a bearer token and drives{" "}
          <code className="font-mono text-xs">/scim/v2/Users</code> to create,
          update and deactivate accounts in this org.
        </span>
        <div className="ml-auto">
          <GatedButton gate="scim_token:create" disabled={!canManage} onClick={() => setIssueOpen(true)}>
            <Plus className="h-4 w-4" />
            Issue token
          </GatedButton>
        </div>
      </div>

      {forbidden && (
        <p className="text-sm text-muted-foreground">
          Provisioning tokens are visible to org admins only. Ask an admin to
          issue or revoke one for your identity provider.
        </p>
      )}
      {tokens.isError && !forbidden && (
        <LoadError
          error={tokens.error}
          resource={t("errors.resources.provisioningTokens")}
          onRetry={() => tokens.refetch()}
        />
      )}
      {revoke.isError && (
        <p className="text-sm text-[color:var(--status-danger-text)]">{(revoke.error as Error).message}</p>
      )}

      {!forbidden && (
        <Table
          columns={columns}
          data={rows as (ScimTokenRow & Record<string, unknown>)[]}
          rowKey="id"
          empty={
            <EmptyState
              uxTarget="provisioning-list"
              icon={<BookUser />}
              title={t("pages.userProvisioning.emptyTitle")}
              description={t("pages.userProvisioning.emptyBody")}
              actions={
                <GatedButton gate="scim_token:create" disabled={!canManage} onClick={() => setIssueOpen(true)}>
                  {t("pages.userProvisioning.emptyAction")}
                </GatedButton>
              }
            />
          }
        />
      )}

      {/* the second half of provisioning: who exists comes from /scim/v2/Users,
          what they may do comes from a mapping written here (#1186) */}
      {orgId && !forbidden && (
        <GroupMappings orgId={orgId} canManage={canManage} />
      )}

      {orgId && (
        <IssueTokenSheet
          open={issueOpen}
          onOpenChange={setIssueOpen}
          orgId={orgId}
          onIssued={invalidate}
        />
      )}

      <Dialog
        open={!!revokeTarget}
        onOpenChange={(open) => !open && setRevokeTarget(null)}
      >
        <DialogHeader>
          <DialogTitle>Revoke provisioning token</DialogTitle>
          <DialogDescription>
            <span className="font-mono">{revokeTarget?.name}</span> stops
            authenticating on the very next request — there is no cache to wait
            out. Accounts it already provisioned are left exactly as they are:
            nobody is deactivated or logged out, the directory simply stops
            syncing until you point the IdP at a new token.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={() => setRevokeTarget(null)}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={revoke.isPending}
            onClick={() => {
              if (!revokeTarget) return;
              const what = revokeTarget.name;
              // this dialog has nowhere to put a failure — it is hand-rolled
              // and carries no error line — so both outcomes toast (#1197)
              revoke.mutate(revokeTarget.id, {
                onSuccess: () => {
                  setRevokeTarget(null);
                  toast.push({ tone: "success", title: t("toast.deleted", { what }) });
                },
                onError: (error) => {
                  toast.push({
                    tone: "error",
                    title: t("toast.deleteFailed", { what }),
                    detail: errorDetail(error),
                  });
                },
              });
            }}
          >
            {revoke.isPending && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            Revoke
          </Button>
        </DialogFooter>
      </Dialog>
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
  const [name, setName] = React.useState("");
  const [issued, setIssued] = React.useState<CreatedScimToken | null>(null);

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
        title={issued ? "Copy the token now" : "Issue provisioning token"}
        subtitle={issued ? issued.name : "scim 2.0 · bearer credential"}
        onClose={() => onOpenChange(false)}
      />
      <SheetBody>
        {issued ? (
          <>
            <div className="rounded-md border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] p-3">
              <div className="flex items-start justify-between gap-2">
                <code
                  data-testid="scim-token-secret"
                  className="min-w-0 break-all font-mono text-sm text-foreground"
                >
                  {issued.secret}
                </code>
                <CopyButton value={issued.secret} label="Copy provisioning token" />
              </div>
            </div>
            <p className="text-sm font-medium text-[color:var(--status-warning-text)]">
              This is the only time this token is shown. Rolter stores a hash of
              it and cannot display or recover it again — if you lose it, issue a
              new token and revoke this one.
            </p>
            <p className="text-sm text-muted-foreground">
              Paste it into your identity provider's SCIM connector as the bearer
              token, alongside the base URL{" "}
              <code className="font-mono text-xs">
                https://your-rolter-host/scim/v2
              </code>
              . The token carries the org, so no tenant id goes in the URL.
            </p>
          </>
        ) : (
          <>
            <Field
              label="Name"
              hint="how you will recognise it later — usually the identity provider it belongs to"
            >
              <Input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Okta production"
              />
            </Field>
            <p className="text-sm text-muted-foreground">
              Provisioned accounts join this org as viewers and have no local
              password: SCIM decides who exists, you still decide what they may
              do.
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
        <div className="flex justify-end gap-2 px-[22px] py-3">
          {issued ? (
            <Button onClick={() => onOpenChange(false)}>Done</Button>
          ) : (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button
                disabled={!name.trim() || create.isPending}
                onClick={() => create.mutate()}
              >
                {create.isPending && (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                )}
                Issue token
              </Button>
            </>
          )}
        </div>
      </SheetFooter>
    </Sheet>
  );
}
