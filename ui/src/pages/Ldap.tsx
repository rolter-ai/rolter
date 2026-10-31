import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Eraser, FolderTree, Loader2, Pencil, Plus, Users } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { GatedSwitch } from "@/components/GatedSwitch";
import { GroupMappings, MAPPABLE_ROLES } from "@/components/GroupMappings";
import { LoadError } from "@/components/LoadError";
import { ListSummary, PageBody, RowIconButton } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Combobox } from "@/components/ui/combobox";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { LabelledValue } from "@/components/ui/labelled-value";
import { CardGridSkeleton } from "@/components/LoadingState";
import { Skeleton } from "@/components/ui/skeleton";
import { SurfacePanel } from "@/components/ui/surface-panel";
import { SwitchRow } from "@/components/ui/switch-row";
import {
  createLdapDirectory,
  createLdapGroupMapping,
  deleteLdapDirectory,
  deleteLdapGroupMapping,
  fetchLdapDirectories,
  fetchLdapGroupMappings,
  updateLdapDirectory,
  type LdapDirectoryInput,
  type LdapDirectoryRow,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { roleLabel } from "@/lib/roles";
import { useScope } from "@/lib/scope";
import { useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";

const DIRECTORIES_KEY = "ldap-directories";
const MAPPINGS_KEY = "ldap-group-mappings";

// the server's defaults for the attribute fields, shown as placeholders and
// sent as-is when a field is left empty
const DEFAULT_FILTER = "(uid={login})";
const DEFAULT_EMAIL_ATTR = "mail";
const DEFAULT_NAME_ATTR = "cn";
const DEFAULT_GROUP_ATTR = "memberOf";

interface Draft {
  name: string;
  url: string;
  bindDn: string;
  bindPassword: string;
  baseDn: string;
  userFilter: string;
  emailAttr: string;
  displayNameAttr: string;
  groupAttr: string;
  requireTls: boolean;
  defaultRole: string;
}

const EMPTY_DRAFT: Draft = {
  name: "",
  url: "",
  bindDn: "",
  bindPassword: "",
  baseDn: "",
  userFilter: "",
  emailAttr: "",
  displayNameAttr: "",
  groupAttr: "",
  requireTls: true,
  defaultRole: "",
};

const draftFrom = (directory: LdapDirectoryRow): Draft => ({
  name: directory.name,
  url: directory.url,
  bindDn: directory.bind_dn,
  // never prefilled: the sealed password is not readable, so an empty field
  // means "keep the stored one", never "clear it"
  bindPassword: "",
  baseDn: directory.base_dn,
  userFilter: directory.user_filter,
  emailAttr: directory.email_attr,
  displayNameAttr: directory.display_name_attr,
  groupAttr: directory.group_attr,
  requireTls: directory.require_tls,
  defaultRole: directory.default_role ?? "",
});

/** the row as the update endpoint wants it back, with `patch` applied */
function inputFrom(
  directory: LdapDirectoryRow,
  patch: Partial<LdapDirectoryInput> = {},
): LdapDirectoryInput {
  return {
    name: directory.name,
    url: directory.url,
    bind_dn: directory.bind_dn,
    base_dn: directory.base_dn,
    user_filter: directory.user_filter,
    email_attr: directory.email_attr,
    display_name_attr: directory.display_name_attr,
    group_attr: directory.group_attr,
    require_tls: directory.require_tls,
    default_role: directory.default_role ?? undefined,
    enabled: directory.enabled,
    ...patch,
  };
}

// register or edit the org's directory. the bind password is sealed on the way
// in and never serialized on the way out, so this form is the only place it is
// legible. clearing it is the card's own control, never a side effect of saving
function DirectorySheet({
  open,
  onOpenChange,
  orgId,
  directory,
  onSaved,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  /** the directory being edited, or null to register one */
  directory: LdapDirectoryRow | null;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const editing = !!directory;
  const initial = React.useMemo(
    () => (directory ? draftFrom(directory) : EMPTY_DRAFT),
    [directory],
  );
  const [draft, setDraft] = React.useState<Draft>(initial);

  React.useEffect(() => {
    if (open) setDraft(initial);
  }, [open, initial]);

  const set = (patch: Partial<Draft>) => setDraft((d) => ({ ...d, ...patch }));

  const save = useMutation({
    mutationFn: () => {
      const input: LdapDirectoryInput = {
        name: draft.name.trim(),
        url: draft.url.trim(),
        bind_dn: draft.bindDn.trim(),
        // trimmed before the check: a stray space must never reach the server
        // as "", which is the wire's "clear it"
        bind_password: draft.bindPassword.trim() || undefined,
        base_dn: draft.baseDn.trim(),
        user_filter: draft.userFilter.trim() || undefined,
        email_attr: draft.emailAttr.trim() || undefined,
        display_name_attr: draft.displayNameAttr.trim() || undefined,
        group_attr: draft.groupAttr.trim() || undefined,
        require_tls: draft.requireTls,
        default_role: draft.defaultRole || undefined,
        enabled: directory ? directory.enabled : true,
      };
      return directory
        ? updateLdapDirectory(directory.id, input)
        : createLdapDirectory(orgId, input);
    },
    onSuccess: () => {
      toast.push({
        tone: "success",
        title: editing ? t("toast.saved") : t("toast.created", { what: draft.name.trim() }),
        detail: editing ? t("toast.savedDetail", { what: draft.name.trim() }) : undefined,
      });
      onSaved();
      onOpenChange(false);
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: draft.name.trim() }),
        detail: errorText(error, t),
      });
    },
  });

  const dirty = editing
    ? (Object.keys(draft) as (keyof Draft)[]).some((k) => draft[k] !== initial[k])
    : (Object.keys(draft) as (keyof Draft)[]).some((k) => draft[k] !== EMPTY_DRAFT[k]);
  // an empty bind DN is a valid directory: the search is anonymous
  const canSave = !!draft.name.trim() && !!draft.url.trim() && !!draft.baseDn.trim();
  // the plain-ldap refusal is the server's, but the form can say it first
  const plainWithTls = draft.requireTls && draft.url.trim().toLowerCase().startsWith("ldap://");

  return (
    <EditorSheet
      name={editing ? "ldap-directory-edit" : "ldap-directory-create"}
      open={open}
      onOpenChange={onOpenChange}
      title={editing ? t("pages.ldap.edit.title") : t("pages.ldap.create.title")}
      subtitle={editing ? t("pages.ldap.edit.subtitle") : t("pages.ldap.create.subtitle")}
      dirty={dirty}
      errorMessage={save.isError ? errorText(save.error, t) : undefined}
      saveLabel={editing ? t("pages.ldap.edit.save") : t("pages.ldap.create.save")}
      canSave={canSave}
      saving={save.isPending}
      onSubmit={() => save.mutate()}
    >
      <Field label={t("pages.ldap.create.name")} hint={t("pages.ldap.create.nameHint")} required>
        <Input
          value={draft.name}
          onChange={(e) => set({ name: e.target.value })}
          placeholder={t("pages.ldap.create.namePlaceholder")}
        />
      </Field>
      <Field label={t("pages.ldap.create.url")} hint={t("pages.ldap.create.urlHint")} required>
        <Input
          value={draft.url}
          onChange={(e) => set({ url: e.target.value })}
          placeholder={t("pages.ldap.create.urlPlaceholder")}
        />
      </Field>
      <SwitchRow
        title={t("pages.ldap.create.requireTls")}
        hint={
          plainWithTls ? t("pages.ldap.create.plainWithTls") : t("pages.ldap.create.requireTlsHint")
        }
        checked={draft.requireTls}
        onChange={(requireTls) => set({ requireTls })}
      />
      <Field label={t("pages.ldap.create.bindDn")} hint={t("pages.ldap.create.bindDnHint")}>
        <Input
          value={draft.bindDn}
          onChange={(e) => set({ bindDn: e.target.value })}
          placeholder={t("pages.ldap.create.bindDnPlaceholder")}
        />
      </Field>
      <Field
        label={t("pages.ldap.create.bindPassword")}
        hint={
          editing
            ? directory?.has_bind_password
              ? t("pages.ldap.edit.bindPasswordHint")
              : t("pages.ldap.edit.bindPasswordNoneHint")
            : t("pages.ldap.create.bindPasswordHint")
        }
      >
        <Input
          type="password"
          autoComplete="new-password"
          value={draft.bindPassword}
          onChange={(e) => set({ bindPassword: e.target.value })}
        />
      </Field>
      <p className="text-sm font-medium text-[color:var(--status-warning-text)]">
        {t("pages.ldap.create.passwordWriteOnly")}
      </p>
      <Field
        label={t("pages.ldap.create.baseDn")}
        hint={t("pages.ldap.create.baseDnHint")}
        required
      >
        <Input
          value={draft.baseDn}
          onChange={(e) => set({ baseDn: e.target.value })}
          placeholder={t("pages.ldap.create.baseDnPlaceholder")}
        />
      </Field>
      <Field label={t("pages.ldap.create.userFilter")} hint={t("pages.ldap.create.userFilterHint")}>
        <Input
          value={draft.userFilter}
          onChange={(e) => set({ userFilter: e.target.value })}
          placeholder={DEFAULT_FILTER}
        />
      </Field>
      <Field label={t("pages.ldap.create.emailAttr")} hint={t("pages.ldap.create.emailAttrHint")}>
        <Input
          value={draft.emailAttr}
          onChange={(e) => set({ emailAttr: e.target.value })}
          placeholder={DEFAULT_EMAIL_ATTR}
        />
      </Field>
      <Field
        label={t("pages.ldap.create.displayNameAttr")}
        hint={t("pages.ldap.create.displayNameAttrHint")}
      >
        <Input
          value={draft.displayNameAttr}
          onChange={(e) => set({ displayNameAttr: e.target.value })}
          placeholder={DEFAULT_NAME_ATTR}
        />
      </Field>
      <Field label={t("pages.ldap.create.groupAttr")} hint={t("pages.ldap.create.groupAttrHint")}>
        <Input
          value={draft.groupAttr}
          onChange={(e) => set({ groupAttr: e.target.value })}
          placeholder={DEFAULT_GROUP_ATTR}
        />
      </Field>
      <Field
        label={t("pages.ldap.create.defaultRole")}
        hint={t("pages.ldap.create.defaultRoleHint")}
      >
        <Combobox
          value={draft.defaultRole}
          onChange={(defaultRole) => set({ defaultRole })}
          options={[
            { value: "", label: t("pages.ldap.create.defaultRoleNone") },
            ...MAPPABLE_ROLES.map((r) => ({ value: r, label: roleLabel(t, r) })),
          ]}
        />
      </Field>
    </EditorSheet>
  );
}

function DirectoryMappings({ directory }: { directory: LdapDirectoryRow }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-col gap-2.5 border-t border-[color:var(--border-subtle)] px-4 py-3.5">
      <div className="flex items-center gap-1.5">
        <Users aria-hidden className="h-3.5 w-3.5 text-muted-foreground" />
        <h3 className="text-2xs uppercase tracking-[0.07em] text-[color:var(--text-subtle)]">
          {t("pages.ldap.mappings.title")}
        </h3>
      </div>
      <GroupMappings
        kind="ldap"
        orgId={directory.org_id}
        queryKey={[MAPPINGS_KEY, directory.id]}
        fetchMappings={() => fetchLdapGroupMappings(directory.id)}
        createMapping={(grant) => createLdapGroupMapping(directory.id, grant)}
        deleteMapping={deleteLdapGroupMapping}
        empty={
          directory.default_role
            ? t("pages.ldap.mappings.emptyWithDefault", {
                role: roleLabel(t, directory.default_role),
              })
            : t("pages.ldap.mappings.empty")
        }
        grantTiming={t("pages.ldap.mappings.grantTiming")}
        removeBody={(role, scope) => t("pages.ldap.mappings.removeBody", { role, scope })}
        addLabel={t("pages.ldap.mappings.addNamed", { directory: directory.name })}
      />
    </div>
  );
}

function DirectoryCard({
  directory,
  onClearPassword,
  onDelete,
  onEdit,
  onToggle,
  clearingPassword,
  deleting,
  toggling,
}: {
  directory: LdapDirectoryRow;
  onClearPassword: (directory: LdapDirectoryRow) => void;
  onDelete: (directory: LdapDirectoryRow) => void;
  onEdit: (directory: LdapDirectoryRow) => void;
  onToggle: (directory: LdapDirectoryRow, enabled: boolean) => void;
  clearingPassword: boolean;
  deleting: boolean;
  toggling: boolean;
}) {
  const { t } = useTranslation();
  return (
    <SurfacePanel>
      <header className="flex flex-wrap items-start gap-x-3 gap-y-2 px-4 py-3.5">
        <FolderTree aria-hidden className="mt-0.5 h-4 w-4 flex-none text-muted-foreground" />
        <div className="min-w-0 flex-1 basis-64">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-foreground">{directory.name}</span>
            {directory.enabled ? (
              <Badge dot tone="success">
                {t("pages.ldap.directory.enabled")}
              </Badge>
            ) : (
              <Badge tone="neutral">{t("pages.ldap.directory.disabled")}</Badge>
            )}
            {!directory.require_tls && (
              <Badge tone="warning" title={t("pages.ldap.directory.noTlsHint")}>
                {t("pages.ldap.directory.noTls")}
              </Badge>
            )}
          </div>
          <p className="mt-1 text-sm text-muted-foreground">
            {directory.default_role
              ? t("pages.ldap.directory.defaultRole", {
                  role: roleLabel(t, directory.default_role),
                })
              : t("pages.ldap.directory.noDefaultRole")}
          </p>
        </div>
        <div className="ml-auto flex flex-none items-center gap-1">
          <GatedSwitch
            gate="ldap_directory:update"
            control="ldap-directory-toggle"
            checked={directory.enabled}
            disabled={toggling}
            onCheckedChange={(next) => onToggle(directory, next)}
            aria-label={t("pages.ldap.directory.toggleNamed", { name: directory.name })}
          />
          <RowIconButton
            gate="ldap_directory:update"
            control="ldap-directory-edit"
            title={t("pages.ldap.directory.edit")}
            aria-label={t("pages.ldap.directory.editNamed", { name: directory.name })}
            onClick={() => onEdit(directory)}
          >
            <Pencil className="h-3.5 w-3.5" />
          </RowIconButton>
          {directory.has_bind_password && (
            <RowIconButton
              gate="ldap_directory:update"
              control="ldap-directory-password-clear"
              title={t("pages.ldap.directory.clearPassword")}
              aria-label={t("pages.ldap.directory.clearPasswordNamed", { name: directory.name })}
              disabled={clearingPassword}
              onClick={() => onClearPassword(directory)}
            >
              {clearingPassword ? (
                <Loader2 className="h-3.5 w-3.5 motion-safe:animate-spin" />
              ) : (
                <Eraser className="h-3.5 w-3.5" />
              )}
            </RowIconButton>
          )}
          <DeleteIconButton
            gate="ldap_directory:delete"
            control="ldap-directory-delete"
            label={t("pages.ldap.directory.deleteNamed", { name: directory.name })}
            title={t("pages.ldap.directory.delete")}
            pending={deleting}
            onClick={() => onDelete(directory)}
          />
        </div>
      </header>

      <div className="flex flex-col gap-1.5 border-t border-[color:var(--border-subtle)] px-4 py-3">
        <LabelledValue label={t("pages.ldap.directory.url")} value={directory.url} />
        <LabelledValue label={t("pages.ldap.directory.bindDn")} value={directory.bind_dn} />
        <LabelledValue
          label={t("pages.ldap.directory.bindPassword")}
          value={
            directory.has_bind_password
              ? t("pages.ldap.directory.passwordStored")
              : t("pages.ldap.directory.passwordMissing")
          }
        />
        <LabelledValue label={t("pages.ldap.directory.baseDn")} value={directory.base_dn} />
        <LabelledValue label={t("pages.ldap.directory.userFilter")} value={directory.user_filter} />
        <LabelledValue label={t("pages.ldap.directory.emailAttr")} value={directory.email_attr} />
        <LabelledValue label={t("pages.ldap.directory.groupAttr")} value={directory.group_attr} />
      </div>

      <DirectoryMappings directory={directory} />
    </SurfacePanel>
  );
}

/**
 * Governance › LDAP (#3261).
 *
 * LDAP sign-in has been an API-only feature since #1826: a directory was
 * registered with a curl. This screen registers, edits, takes in and out of
 * service and deletes the org's one directory, and maps its groups to roles.
 */
export default function Ldap() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const scope = useScope();
  const orgId = scope.orgId;

  const directories = useQuery({
    queryKey: [DIRECTORIES_KEY, orgId],
    queryFn: () => fetchLdapDirectories(orgId as string),
    enabled: !!orgId,
    retry: false,
  });
  useScreenReady(!directories.isLoading);

  const invalidate = () => queryClient.invalidateQueries({ queryKey: [DIRECTORIES_KEY, orgId] });

  const remove = useMutation({
    mutationFn: (id: string) => deleteLdapDirectory(id),
    onSuccess: invalidate,
  });

  // the switch on a card sends the row back unchanged except for `enabled`,
  // and omits `bind_password` so the sealed one is left alone
  const toggle = useMutation({
    mutationFn: ({ directory, enabled }: { directory: LdapDirectoryRow; enabled: boolean }) =>
      updateLdapDirectory(directory.id, inputFrom(directory, { enabled })),
    onSuccess: (updated) => {
      invalidate();
      toast.push({
        tone: "success",
        title: updated.enabled
          ? t("pages.ldap.directory.enabledToast", { name: updated.name })
          : t("pages.ldap.directory.disabledToast", { name: updated.name }),
      });
    },
    onError: (error, { directory }) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: directory.name }),
        detail: errorText(error, t),
      });
    },
  });

  // the deliberate way to drop a sealed bind password: the empty string is the
  // third value the update understands, and nothing else here sends it
  const clearPassword = useMutation({
    mutationFn: (directory: LdapDirectoryRow) =>
      updateLdapDirectory(directory.id, inputFrom(directory, { bind_password: "" })),
    onSuccess: invalidate,
  });

  const [sheetOpen, setSheetOpen] = React.useState(false);
  const [editing, setEditing] = React.useState<LdapDirectoryRow | null>(null);
  const openCreate = () => {
    setEditing(null);
    setSheetOpen(true);
  };
  const openEdit = (directory: LdapDirectoryRow) => {
    setEditing(directory);
    setSheetOpen(true);
  };
  const [deleteTarget, setDeleteTarget] = React.useState<LdapDirectoryRow | null>(null);
  const [disableTarget, setDisableTarget] = React.useState<LdapDirectoryRow | null>(null);
  const [passwordTarget, setPasswordTarget] = React.useState<LdapDirectoryRow | null>(null);
  const switchDirectory = (directory: LdapDirectoryRow, enabled: boolean) => {
    if (enabled) {
      toggle.mutate({ directory, enabled });
      return;
    }
    toggle.reset();
    setDisableTarget(directory);
  };

  if (scope.isLoading || (!!orgId && directories.isLoading)) {
    return (
      <PageBody>
        <Skeleton className="h-9 w-[280px] rounded-md" />
        <CardGridSkeleton cards={1} height={240} min={420} />
      </PageBody>
    );
  }

  const rows = directories.data ?? [];
  const canManage = !!orgId && !directories.isError;
  // an org has one directory, so a second is not offered; the control plane
  // refuses it too, for a race between two admins
  const hasDirectory = rows.length > 0;

  return (
    <PageBody>
      <div className="flex flex-wrap items-center gap-3">
        <h2 className="text-sm font-medium text-foreground">{t("pages.ldap.directory.title")}</h2>
        <ListSummary data={directories.data}>
          {(all) => t("pages.ldap.directory.count", { count: all.length })}
        </ListSummary>
        <GatedButton
          gate="ldap_directory:create"
          control="ldap-directory-new"
          className="ml-auto"
          disabled={!canManage || hasDirectory}
          title={hasDirectory ? t("pages.ldap.directory.oneOnly") : undefined}
          onClick={openCreate}
        >
          <Plus className="h-4 w-4" aria-hidden />
          {t("pages.ldap.directory.add")}
        </GatedButton>
      </div>

      {directories.isError && (
        <LoadError
          error={directories.error}
          resource={t("errors.resources.ldapDirectories")}
          onRetry={() => directories.refetch()}
          target="ldap-directories"
        />
      )}

      {!directories.isError &&
        (rows.length === 0 ? (
          <EmptyState
            thread
            uxTarget="ldap-directories"
            icon={<FolderTree />}
            title={t("pages.ldap.empty.title")}
            description={t("pages.ldap.empty.body")}
            actions={
              <GatedButton
                gate="ldap_directory:create"
                control="ldap-directory-new-empty"
                disabled={!canManage}
                onClick={openCreate}
              >
                <Plus className="h-4 w-4" aria-hidden />
                {t("pages.ldap.directory.add")}
              </GatedButton>
            }
          />
        ) : (
          <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(420px,100%),1fr))]">
            {rows.map((directory) => (
              <DirectoryCard
                key={directory.id}
                directory={directory}
                clearingPassword={
                  clearPassword.isPending && clearPassword.variables?.id === directory.id
                }
                deleting={remove.isPending && remove.variables === directory.id}
                toggling={toggle.isPending && toggle.variables?.directory.id === directory.id}
                onClearPassword={(target) => {
                  clearPassword.reset();
                  setPasswordTarget(target);
                }}
                onDelete={(target) => {
                  remove.reset();
                  setDeleteTarget(target);
                }}
                onEdit={openEdit}
                onToggle={switchDirectory}
              />
            ))}
          </div>
        ))}

      {orgId && (
        <DirectorySheet
          open={sheetOpen}
          onOpenChange={setSheetOpen}
          orgId={orgId}
          directory={editing}
          onSaved={invalidate}
        />
      )}

      <ConfirmDialog
        name="ldap-password-clear"
        open={!!passwordTarget}
        onOpenChange={(open) => !open && setPasswordTarget(null)}
        title={t("pages.ldap.clearPassword.title", { name: passwordTarget?.name })}
        description={t("pages.ldap.clearPassword.body")}
        confirmLabel={t("pages.ldap.clearPassword.confirm")}
        pending={clearPassword.isPending}
        error={clearPassword.error}
        onConfirm={() => {
          if (!passwordTarget) return;
          const name = passwordTarget.name;
          clearPassword.mutate(passwordTarget, {
            onSuccess: () => {
              setPasswordTarget(null);
              toast.push({
                tone: "success",
                title: t("pages.ldap.directory.passwordClearedToast", { name }),
              });
            },
          });
        }}
      />

      {/* out of service is reversible with one flip, so it confirms as a
          default-tone action; the mutation is reset on close so a refusal for
          one directory does not greet the next */}
      <ConfirmDialog
        name="ldap-directory-disable"
        open={!!disableTarget}
        onOpenChange={(open) => {
          if (open) return;
          setDisableTarget(null);
          toggle.reset();
        }}
        title={t("pages.ldap.disable.title", { name: disableTarget?.name })}
        description={t("pages.ldap.disable.body")}
        confirmLabel={t("pages.ldap.disable.confirm")}
        pending={toggle.isPending}
        error={toggle.error}
        onConfirm={() => {
          if (!disableTarget) return;
          toggle.mutate(
            { directory: disableTarget, enabled: false },
            { onSuccess: () => setDisableTarget(null) },
          );
        }}
      />

      <ConfirmDialog
        name="ldap-directory-delete"
        open={!!deleteTarget}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("pages.ldap.confirm.title", { name: deleteTarget?.name })}
        description={t("pages.ldap.confirm.body")}
        confirmLabel={t("pages.ldap.confirm.confirm")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => {
          if (!deleteTarget) return;
          const what = deleteTarget.name;
          remove.mutate(deleteTarget.id, {
            onSuccess: () => {
              setDeleteTarget(null);
              toast.push({ tone: "success", title: t("toast.deleted", { what }) });
            },
          });
        }}
      />
    </PageBody>
  );
}
