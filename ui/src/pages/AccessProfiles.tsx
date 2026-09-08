import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2, Pencil, Shield, Trash2, Users } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { EditorSheet } from "@/components/EditorSheet";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { CardGridSkeleton } from "@/components/LoadingState";
import { PageBody, Pill, Toolbar } from "@/components/screen";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  createAccessProfile,
  deleteAccessProfile,
  fetchAccessProfile,
  fetchAccessProfiles,
  fetchCustomRoles,
  updateAccessProfile,
  type AccessProfileDetail,
  type AccessProfilePolicyInput,
  type AccessProfileRow,
  type CustomRoleRow,
} from "@/lib/api";
import { useScope } from "@/lib/scope";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

/// One profile card, with everything the profile actually carries.
///
/// `GET /api/v1/access-profiles/{id}` answers roles, assignments and policy in
/// one call, which is why the card reads it instead of the three separate lists
/// (#1184): the policy in particular has no list endpoint at all, so a profile's
/// model rules could be written through `setAccessProfilePolicy` and never shown
/// back. Fetched per profile because the API scopes it that way; a deployment
/// has few profiles, so this stays a handful of requests rather than a fan-out
/// worth batching.
function ProfileCard({
  profile,
  onEdit,
  onDelete,
  deleting,
}: {
  profile: AccessProfileRow;
  onEdit: (detail: AccessProfileDetail) => void;
  onDelete: (profile: AccessProfileRow) => void;
  deleting: boolean;
}) {
  const { t } = useTranslation();
  const detail = useQuery({
    queryKey: ["access-profile", profile.id],
    queryFn: () => fetchAccessProfile(profile.id),
    retry: false,
  });

  const users = detail.data?.assignments.filter((a) => a.user_id).length ?? 0;
  const teams = detail.data?.assignments.filter((a) => a.team_id).length ?? 0;
  const roles = detail.data?.roles.length ?? 0;
  const policy = detail.data?.policy;
  const rules = policy
    ? policy.allowed_models.length +
      policy.denied_models.length +
      policy.allowed_routes.length +
      policy.denied_routes.length
    : 0;

  return (
    <div className="rounded-lg border p-4">
      <div className="flex items-start gap-3">
        <Shield className="mt-0.5 size-4 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="font-medium">{profile.name}</span>
            <Pill color="var(--text-secondary)" tint="var(--surface-subtle)">
              {profile.slug}
            </Pill>
          </div>
          {profile.description && (
            <p className="mt-1 text-sm text-muted-foreground">
              {profile.description}
            </p>
          )}
          <div className="mt-2 flex items-center gap-1.5 text-sm text-muted-foreground">
            <Users className="size-3.5" />
            {detail.isLoading ? (
              <span>{t("common.loading")}</span>
            ) : detail.isError ? (
              <span>{t("pages.accessProfiles.detailUnavailable")}</span>
            ) : (
              // a profile that reaches nobody grants nothing, which is worth
              // saying plainly rather than showing as "0 users, 0 teams"
              <span>
                {users === 0 && teams === 0
                  ? t("pages.accessProfiles.unassigned")
                  : `${t("pages.accessProfiles.userCount", { count: users })} · ${t("pages.accessProfiles.teamCount", { count: teams })}`}
              </span>
            )}
          </div>
          {detail.data && (
            <div className="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
              <span>{t("pages.accessProfiles.roleCount", { count: roles })}</span>
              <span aria-hidden>·</span>
              <span>
                {rules === 0
                  ? t("pages.accessProfiles.policyNone")
                  : t("pages.accessProfiles.policyCount", { count: rules })}
              </span>
            </div>
          )}
        </div>
        <div className="flex flex-none items-center gap-1">
          <GatedButton
            gate="access_profile:update"
            variant="ghost"
            size="icon"
            title={t("pages.accessProfiles.editProfile", { name: profile.name })}
            aria-label={t("pages.accessProfiles.editProfile", { name: profile.name })}
            disabled={!detail.data}
            onClick={() => detail.data && onEdit(detail.data)}
          >
            <Pencil className="size-4" />
          </GatedButton>
          <GatedButton
            gate="access_profile:delete"
            variant="ghost"
            size="icon"
            title={t("pages.accessProfiles.deleteProfile", { name: profile.name })}
            aria-label={t("pages.accessProfiles.deleteProfile", { name: profile.name })}
            disabled={deleting}
            onClick={() => onDelete(profile)}
          >
            {deleting ? (
              <Loader2 className="size-4 animate-spin" />
            ) : (
              <Trash2 className="size-4" />
            )}
          </GatedButton>
        </div>
      </div>
    </div>
  );
}

/** The editor's working copy of one profile. */
interface ProfileDraft {
  /** absent while creating */
  id?: string;
  name: string;
  slug: string;
  description: string;
  /** the custom roles composed into it, at org scope */
  roleIds: string[];
  /** one pattern per line, exactly as typed */
  allowedModels: string;
  deniedModels: string;
  allowedRoutes: string;
  deniedRoutes: string;
}

const emptyDraft = (): ProfileDraft => ({
  name: "",
  slug: "",
  description: "",
  roleIds: [],
  allowedModels: "",
  deniedModels: "",
  allowedRoutes: "",
  deniedRoutes: "",
});

const draftFrom = (detail: AccessProfileDetail): ProfileDraft => ({
  id: detail.id,
  name: detail.name,
  slug: detail.slug,
  description: detail.description ?? "",
  roleIds: detail.roles.map((r) => r.role_id),
  allowedModels: (detail.policy?.allowed_models ?? []).join("\n"),
  deniedModels: (detail.policy?.denied_models ?? []).join("\n"),
  allowedRoutes: (detail.policy?.allowed_routes ?? []).join("\n"),
  deniedRoutes: (detail.policy?.denied_routes ?? []).join("\n"),
});

/** One pattern per line; the control plane trims and drops the blanks anyway. */
const patterns = (raw: string): string[] =>
  raw
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);

const policyOf = (draft: ProfileDraft): AccessProfilePolicyInput => ({
  allowed_models: patterns(draft.allowedModels),
  denied_models: patterns(draft.deniedModels),
  allowed_routes: patterns(draft.allowedRoutes),
  denied_routes: patterns(draft.deniedRoutes),
});

export default function AccessProfiles() {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const scope = useScope();
  const orgId = scope.orgId as string | undefined;

  const profiles = useQuery({
    queryKey: ["access-profiles", orgId],
    queryFn: () => fetchAccessProfiles(orgId as string),
    enabled: !!orgId,
  });

  // the roles a profile can compose. a profile may only carry roles from its own
  // org, so this is the same org the profiles were listed for
  const roles = useQuery({
    queryKey: ["custom-roles", orgId],
    queryFn: () => fetchCustomRoles(orgId as string),
    enabled: !!orgId,
    retry: false,
  });

  useScreenReady(!profiles.isLoading);
  useErrorState(!!profiles.error, "access-profiles");

  const invalidate = (id?: string) => {
    void queryClient.invalidateQueries({ queryKey: ["access-profiles", orgId] });
    if (id) void queryClient.invalidateQueries({ queryKey: ["access-profile", id] });
  };

  const save = useMutation({
    mutationFn: (draft: ProfileDraft) => {
      // roles and policy replace wholesale, so both go in the one request that
      // saves the profile — a profile is never assignable half-written
      const roleBodies = draft.roleIds.map((role_id) => ({ role_id }));
      return draft.id
        ? updateAccessProfile(draft.id, {
            name: draft.name.trim(),
            description: draft.description.trim() || null,
            roles: roleBodies,
            policy: policyOf(draft),
          })
        : createAccessProfile(orgId as string, {
            name: draft.name.trim(),
            slug: draft.slug.trim() || undefined,
            description: draft.description.trim() || null,
            roles: roleBodies,
            policy: policyOf(draft),
          });
    },
    onSuccess: (saved, draft) => {
      invalidate(saved.id);
      setDraft(null);
      toast.push({
        tone: "success",
        title: draft.id ? t("toast.saved") : t("toast.created", { what: saved.name }),
      });
    },
    onError: (error, draft) =>
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: draft.name.trim() }),
        detail: errorDetail(error),
      }),
  });

  const remove = useMutation({
    mutationFn: (profile: AccessProfileRow) => deleteAccessProfile(profile.id),
    onSuccess: (_void, profile) => {
      invalidate();
      setDeleteTarget(null);
      toast.push({ tone: "success", title: t("toast.deleted", { what: profile.name }) });
    },
    onError: (error, profile) =>
      toast.push({
        tone: "error",
        title: t("toast.deleteFailed", { what: profile.name }),
        detail: errorDetail(error),
      }),
  });

  const [draft, setDraft] = React.useState<ProfileDraft | null>(null);
  const [seed, setSeed] = React.useState("");
  // a profile reaches users and teams, so deleting it changes what a group of
  // people can do — confirmed by name first (#1179)
  const [deleteTarget, setDeleteTarget] = React.useState<AccessProfileRow | null>(
    null,
  );
  const startDelete = (profile: AccessProfileRow) => {
    remove.reset();
    setDeleteTarget(profile);
  };

  const startCreate = () => {
    save.reset();
    setDraft(emptyDraft());
    setSeed(JSON.stringify(emptyDraft()));
  };
  const startEdit = (detail: AccessProfileDetail) => {
    save.reset();
    const next = draftFrom(detail);
    setDraft(next);
    setSeed(JSON.stringify(next));
  };

  return (
    <PageBody>
      <Toolbar>
        <span className="text-sm text-muted-foreground">
          {t("pages.accessProfiles.summary", { count: profiles.data?.length ?? 0 })}
        </span>
        <GatedButton gate="access_profile:create" className="ml-auto" disabled={!orgId} onClick={startCreate}>
          {t("pages.accessProfiles.add")}
        </GatedButton>
      </Toolbar>

      {profiles.isLoading && <CardGridSkeleton cards={3} height={196} min={380} />}
      {profiles.isError && (
        <LoadError
          error={profiles.error}
          resource={t("errors.resources.accessProfiles")}
          onRetry={() => void profiles.refetch()}
        />
      )}
      {profiles.data && profiles.data.length === 0 && (
        <EmptyState
          icon={<Shield className="size-5" />}
          title={t("pages.accessProfiles.emptyTitle")}
          description={t("pages.accessProfiles.emptyBody")}
          uxTarget="access-profiles"
          actions={
            <GatedButton gate="access_profile:create" disabled={!orgId} onClick={startCreate}>
              {t("pages.accessProfiles.emptyAction")}
            </GatedButton>
          }
        />
      )}

      <div className="grid gap-3.5 [grid-template-columns:repeat(auto-fill,minmax(min(380px,100%),1fr))]">
        {(profiles.data ?? []).map((profile) => (
          <ProfileCard
            key={profile.id}
            profile={profile}
            deleting={remove.isPending && remove.variables?.id === profile.id}
            onEdit={startEdit}
            onDelete={startDelete}
          />
        ))}
      </div>

      <ConfirmDialog
        open={!!deleteTarget}
        onOpenChange={(o) => !o && setDeleteTarget(null)}
        title={t("pages.accessProfiles.confirm.title", { name: deleteTarget?.name })}
        description={t("pages.accessProfiles.confirm.body")}
        confirmLabel={t("pages.accessProfiles.confirm.confirm")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => deleteTarget && remove.mutate(deleteTarget)}
      />

      {draft && (
        <ProfileSheet
          draft={draft}
          dirty={JSON.stringify(draft) !== seed}
          roles={roles.data ?? []}
          saving={save.isPending}
          errorMessage={save.error ? errorDetail(save.error) : undefined}
          onChange={setDraft}
          onClose={() => setDraft(null)}
          onSave={() => save.mutate(draft)}
        />
      )}
    </PageBody>
  );
}

function ProfileSheet({
  draft,
  dirty,
  roles,
  saving,
  errorMessage,
  onChange,
  onClose,
  onSave,
}: {
  draft: ProfileDraft;
  dirty: boolean;
  roles: CustomRoleRow[];
  saving: boolean;
  errorMessage?: string;
  onChange: (draft: ProfileDraft) => void;
  onClose: () => void;
  onSave: () => void;
}) {
  const { t } = useTranslation();

  const toggleRole = (id: string) =>
    onChange({
      ...draft,
      roleIds: draft.roleIds.includes(id)
        ? draft.roleIds.filter((r) => r !== id)
        : [...draft.roleIds, id],
    });

  const policyField = (
    label: string,
    hint: string,
    key: "allowedModels" | "deniedModels" | "allowedRoutes" | "deniedRoutes",
    placeholder: string,
  ) => (
    <Field label={label} hint={hint}>
      <Textarea
        rows={3}
        value={draft[key]}
        placeholder={placeholder}
        onChange={(e) => onChange({ ...draft, [key]: e.target.value })}
      />
    </Field>
  );

  return (
    <EditorSheet
      open
      onOpenChange={(open) => !open && onClose()}
      title={
        draft.id
          ? t("pages.accessProfiles.editTitle", { name: draft.name })
          : t("pages.accessProfiles.createTitle")
      }
      subtitle={t("pages.accessProfiles.sheetSubtitle")}
      dirty={dirty}
      errorMessage={errorMessage}
      saveLabel={
        draft.id ? t("pages.accessProfiles.save") : t("pages.accessProfiles.create")
      }
      canSave={!!draft.name.trim()}
      saving={saving}
      onSave={onSave}
    >
      <div className="space-y-3.5">
        <Field label={t("pages.accessProfiles.fieldName")}>
          <Input
            value={draft.name}
            placeholder={t("pages.accessProfiles.namePlaceholder")}
            onChange={(e) => onChange({ ...draft, name: e.target.value })}
          />
        </Field>
        {/* the slug is the profile's stable handle; the control plane derives it
            once and never renames it, so it is offered on create alone */}
        {!draft.id && (
          <Field
            label={t("pages.accessProfiles.fieldSlug")}
            hint={t("pages.accessProfiles.slugHint")}
          >
            <Input
              value={draft.slug}
              placeholder={t("pages.accessProfiles.slugPlaceholder")}
              onChange={(e) => onChange({ ...draft, slug: e.target.value })}
            />
          </Field>
        )}
        <Field label={t("pages.accessProfiles.fieldDescription")}>
          <Input
            value={draft.description}
            placeholder={t("pages.accessProfiles.descriptionPlaceholder")}
            onChange={(e) => onChange({ ...draft, description: e.target.value })}
          />
        </Field>

        <fieldset className="space-y-2">
          <legend className="text-sm font-medium leading-none">
            {t("pages.accessProfiles.rolesLabel")}
          </legend>
          <p className="text-xs text-muted-foreground">
            {t("pages.accessProfiles.rolesHint")}
          </p>
          {roles.length === 0 ? (
            <p className="rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-3.5 py-2.5 text-xs text-muted-foreground">
              {t("pages.accessProfiles.rolesEmpty")}
            </p>
          ) : (
            <div className="grid gap-2 sm:grid-cols-2">
              {roles.map((role) => (
                <label
                  key={role.id}
                  className="flex cursor-pointer items-start gap-2 rounded-lg border border-[color:var(--border-subtle)] p-2.5 text-sm hover:bg-[color:var(--surface-hover)]"
                >
                  <input
                    type="checkbox"
                    className="mt-0.5 h-4 w-4 accent-[color:var(--red-folk)]"
                    checked={draft.roleIds.includes(role.id)}
                    onChange={() => toggleRole(role.id)}
                  />
                  <span className="min-w-0">
                    <span className="block truncate font-medium">{role.name}</span>
                    <span className="text-xs text-muted-foreground">
                      {t("pages.accessProfiles.roleExtends", { role: role.base_role })}
                    </span>
                  </span>
                </label>
              ))}
            </div>
          )}
        </fieldset>

        <fieldset className="space-y-2.5">
          <legend className="text-sm font-medium leading-none">
            {t("pages.accessProfiles.policyLabel")}
          </legend>
          <p className="text-xs text-muted-foreground">
            {t("pages.accessProfiles.policyHint")}
          </p>
          {policyField(
            t("pages.accessProfiles.allowedModels"),
            t("pages.accessProfiles.allowHint"),
            "allowedModels",
            "gpt-4o",
          )}
          {policyField(
            t("pages.accessProfiles.deniedModels"),
            t("pages.accessProfiles.denyHint"),
            "deniedModels",
            "o1-*",
          )}
          {policyField(
            t("pages.accessProfiles.allowedRoutes"),
            t("pages.accessProfiles.allowHint"),
            "allowedRoutes",
            "chat-*",
          )}
          {policyField(
            t("pages.accessProfiles.deniedRoutes"),
            t("pages.accessProfiles.denyHint"),
            "deniedRoutes",
            "internal-*",
          )}
        </fieldset>
      </div>
    </EditorSheet>
  );
}
