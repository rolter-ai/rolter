import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { Loader2, Trash2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { LoadingRegion } from "@/components/LoadingState";
import {
  ORG_TARGET,
  orgScopeText,
  OrgScopePicker,
  OrgScopePill,
  scopeTargetIds,
  useOrgScope,
  type ScopeTarget,
} from "@/components/OrgScopePicker";
import { RowIconButton } from "@/components/screen";
import { Badge } from "@/components/ui/badge";
import { Combobox } from "@/components/ui/combobox";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { ROLES } from "@/lib/api";
import type { Capability } from "@/lib/can";
import { useToast } from "@/lib/toast";

// the roles a group mapping may grant, mirroring `parse_role` in
// crates/rolter-control/src/sso.rs, which scim_groups.rs calls verbatim. not
// /api/v1/roles: that list carries every role the control plane knows about,
// and offering one the endpoint refuses would build a form that can only fail
// on submit
export const MAPPABLE_ROLES = ROLES;

// the role a new mapping starts on: the least powerful one, chosen here rather
// than read off `ROLES`, whose order other screens depend on (#2078)
const STARTING_ROLE = "viewer";

// the label for a role the server sent us, falling back to the raw value so a
// newer control plane's role is shown rather than rendered as a missing key
export function roleLabel(t: TFunction, role: string): string {
  return t(`shell.roles.${role}`, { defaultValue: role });
}

// the capability each kind of mapping is gated on, and the one the control
// plane's table (`rbac_matrix.rs`) writes for it
const GATES = {
  scim: { create: "scim_group_mapping:create", delete: "scim_group_mapping:delete" },
  sso: { create: "sso_group_mapping:create", delete: "sso_group_mapping:delete" },
} as const satisfies Record<string, Record<string, Capability>>;

/** what the list shows of a mapping: both APIs answer with these and more */
export interface GroupMappingRow {
  id: string;
  group_name: string;
  role: string;
  /** the most specific non-null scope wins; both null grants at the org */
  team_id: string | null;
  project_id: string | null;
}

/** what a mapping is written with, which both create endpoints accept */
export interface GroupMappingGrant {
  group_name: string;
  role: string;
  team_id?: string;
  project_id?: string;
}

// one mapping as the form holds it, the moment it is sent or asked about
interface Draft {
  group: string;
  role: string;
  target: ScopeTarget;
}

type GrantReason = "admin" | "org";

/**
 * Why writing this mapping is worth a second look, or nothing when it is not.
 *
 * A mapping hands a role to everyone in an identity-provider group at once, so
 * the two grants that reach furthest are asked about first: the top role, and
 * any role across the whole organization rather than one team or project. The
 * dialog names which of the two raised it, since "admin" on one team and
 * "viewer" on everything are different questions.
 */
function grantReasons({ role, target }: Pick<Draft, "role" | "target">): GrantReason[] {
  const reasons: GrantReason[] = [];
  if (role === "admin") reasons.push("admin");
  if (target === ORG_TARGET) reasons.push("org");
  return reasons;
}

export interface GroupMappingsProps {
  /**
   * SCIM or SSO: picks the capability each control is gated on and the name the
   * confirmations report under. The control slugs are the same for both, since
   * a refused click also records the capability, which says which one it was.
   */
  kind: keyof typeof GATES;
  /** the org a mapping grants inside: the picker lists its teams and projects */
  orgId: string;
  /** the list's react-query key; a write invalidates it */
  queryKey: readonly unknown[];
  fetchMappings: () => Promise<GroupMappingRow[]>;
  createMapping: (grant: GroupMappingGrant) => Promise<unknown>;
  deleteMapping: (id: string) => Promise<unknown>;
  /** already translated: what "no mapping yet" means on this screen */
  empty: string;
  /** already translated: when a new mapping reaches the people already in the group */
  grantTiming: string;
  /** already translated: what removing a mapping does, given its role and scope */
  removeBody: (role: string, scope: string) => string;
  /** already translated: names the add button where several forms share a screen */
  addLabel?: string;
}

/**
 * The IdP groups an org turns into roles, with the form that writes one (#1186).
 *
 * Both the SCIM and the single sign-on screens map a group to a role at the
 * org, a team or a project, and both used to carry their own copy of this. They
 * differ only in which endpoints they call and what their copy promises about
 * timing, so those arrive as props and the rest is here.
 *
 * A mapping is the control that grants roles to groups of people at once, so
 * the form starts on the least powerful role (#2078) and a mapping that grants
 * admin, or grants at the whole org, is confirmed first, naming the group, the
 * role and the scope. After a mapping is written the form goes back to its
 * start, so the next one does not inherit what the last one was granted.
 *
 * The scope select is the shared `OrgScopePicker`: the org, every team in it,
 * and every project in any of those teams, so a mapping onto a project in a
 * team the scope switcher does not currently have selected can be written
 * without moving the switcher first (#1249). A mapping may never grant outside
 * its own org, which the control plane enforces whatever this sends.
 */
export function GroupMappings({
  kind,
  orgId,
  queryKey,
  fetchMappings,
  createMapping,
  deleteMapping,
  empty,
  grantTiming,
  removeBody,
  addLabel,
}: GroupMappingsProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  // the mapping's own org, not the scope switcher's: a mapping can only ever
  // reach inside the org that owns it
  const scope = useOrgScope(orgId);
  const gate = GATES[kind];

  const mappings = useQuery({ queryKey, queryFn: fetchMappings, retry: false });
  const rows = mappings.data ?? [];

  const invalidate = () => queryClient.invalidateQueries({ queryKey });

  const [group, setGroup] = React.useState("");
  const [role, setRole] = React.useState<string>(STARTING_ROLE);
  // "" is the org; otherwise "team:<id>" or "project:<id>"
  const [target, setTarget] = React.useState<ScopeTarget>(ORG_TARGET);
  // the mapping waiting on an answer, when it is one worth asking about
  const [confirming, setConfirming] = React.useState<Draft | null>(null);

  const create = useMutation({
    mutationFn: (draft: Draft) =>
      createMapping({
        group_name: draft.group,
        role: draft.role,
        ...scopeTargetIds(draft.target),
      }),
    // the failure stays beside what caused it, in the form or in the dialog;
    // the success is what would otherwise be silent, so that is the one that
    // toasts (#1197)
    onSuccess: (_created, draft) => {
      setGroup("");
      setRole(STARTING_ROLE);
      setTarget(ORG_TARGET);
      setConfirming(null);
      invalidate();
      toast.push({ tone: "success", title: t("toast.created", { what: draft.group }) });
    },
  });

  const submit = () => {
    const draft: Draft = { group: group.trim(), role, target };
    if (grantReasons(draft).length === 0) {
      create.mutate(draft);
      return;
    }
    create.reset();
    setConfirming(draft);
  };

  const remove = useMutation({
    mutationFn: (id: string) => deleteMapping(id),
    onSuccess: invalidate,
  });

  // a mapping is what puts people in a role, so removing one takes access away
  // from everyone in that group: named and confirmed first (#1179)
  const [removeTarget, setRemoveTarget] = React.useState<GroupMappingRow | null>(null);
  const startRemove = (mapping: GroupMappingRow) => {
    remove.reset();
    setRemoveTarget(mapping);
  };

  const reasons = confirming ? grantReasons(confirming) : [];

  return (
    <div className="flex flex-col gap-2.5">
      {mappings.isLoading && (
        <LoadingRegion testId="group-mappings-loading">
          <Skeleton height={36} radius={6} />
        </LoadingRegion>
      )}
      {mappings.isError && (
        <LoadError
          error={mappings.error}
          resource={t("errors.resources.groupMappings")}
          onRetry={() => mappings.refetch()}
          target="group-mappings"
        />
      )}
      {mappings.isSuccess && rows.length === 0 && (
        <p className="text-sm text-muted-foreground">{empty}</p>
      )}

      {rows.length > 0 && (
        <ul className="flex flex-col gap-1.5">
          {rows.map((mapping) => (
            <li
              key={mapping.id}
              className="flex flex-wrap items-center gap-x-2 gap-y-1 rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-2.5 py-1.5"
            >
              {/* the name gets a line to itself on a phone: sharing one with the
                  scope chip and the role left it a few characters, or none */}
              <span className="min-w-0 basis-full truncate font-mono text-xs text-foreground sm:flex-1 sm:basis-0">
                {mapping.group_name}
              </span>
              <OrgScopePill scope={scope} value={mapping} />
              <Badge tone="neutral">{roleLabel(t, mapping.role)}</Badge>
              <RowIconButton
                danger
                className="ml-auto"
                gate={gate.delete}
                control="mapping-remove"
                title={t("groupMappings.remove")}
                aria-label={t("groupMappings.removeNamed", { group: mapping.group_name })}
                disabled={remove.isPending}
                onClick={() => startRemove(mapping)}
              >
                <Trash2 className="h-3.5 w-3.5" />
              </RowIconButton>
            </li>
          ))}
        </ul>
      )}

      {/* a labelled control each, stacked on a phone where the row would leave
          the group name three characters wide, side by side from `sm`. the
          button's top margin is the label's line and the gap under it, so it
          sits level with the controls even when the scope picker grows a line
          of its own underneath */}
      <div className="flex flex-col gap-3 sm:flex-row sm:flex-wrap sm:items-start">
        <Field
          label={t("groupMappings.groupLabel")}
          className="sm:max-w-[22rem] sm:flex-[1_1_14rem]"
        >
          <Input
            className="h-8"
            value={group}
            onChange={(e) => setGroup(e.target.value)}
            placeholder={t("groupMappings.groupPlaceholder")}
          />
        </Field>
        <Field label={t("groupMappings.scopeLabel")} className="sm:w-[196px]">
          <OrgScopePicker
            orgId={orgId}
            value={target}
            onChange={setTarget}
            label={t("groupMappings.scopeLabel")}
            className="w-full"
          />
        </Field>
        <Field label={t("groupMappings.roleLabel")} className="sm:w-40">
          <Combobox
            size="sm"
            className="w-full"
            value={role}
            onChange={setRole}
            options={MAPPABLE_ROLES.map((r) => ({ value: r, label: roleLabel(t, r) }))}
          />
        </Field>
        <GatedButton
          gate={gate.create}
          control="mapping-add"
          size="sm"
          variant="outline"
          className="w-full sm:mt-5 sm:w-auto"
          aria-label={addLabel}
          disabled={!group.trim() || create.isPending}
          onClick={submit}
        >
          {create.isPending && !confirming && (
            <Loader2 className="h-4 w-4 motion-safe:animate-spin" aria-hidden />
          )}
          {t("groupMappings.add")}
        </GatedButton>
      </div>
      {/* the control plane's own message, never a gloss on it. while the
          dialog is up it carries the refusal itself */}
      {create.isError && !confirming && (
        <p role="alert" className="text-sm text-[color:var(--status-danger-text)]">
          {(create.error as Error).message}
        </p>
      )}

      <ConfirmDialog
        name={`${kind}-group-mapping-grant`}
        tone="default"
        open={!!confirming}
        onOpenChange={(open) => {
          if (open) return;
          setConfirming(null);
          // a refusal from this attempt would otherwise greet the next one
          create.reset();
        }}
        title={t("groupMappings.grant.title", {
          group: confirming?.group,
          role: confirming ? roleLabel(t, confirming.role) : "",
        })}
        description={[
          confirming?.target === ORG_TARGET
            ? t("groupMappings.grant.bodyOrg", {
                group: confirming.group,
                role: roleLabel(t, confirming.role),
              })
            : t("groupMappings.grant.bodyScoped", {
                group: confirming?.group,
                role: confirming ? roleLabel(t, confirming.role) : "",
                scope: confirming ? orgScopeText(t, scope, scopeTargetIds(confirming.target)) : "",
              }),
          grantTiming,
        ].join(" ")}
        confirmLabel={t("groupMappings.add")}
        pending={create.isPending}
        error={create.error}
        onConfirm={() => confirming && create.mutate(confirming)}
      >
        <ul className="list-disc space-y-1 pl-5 text-sm text-muted-foreground">
          {reasons.includes("admin") && <li>{t("groupMappings.grant.reasonAdmin")}</li>}
          {reasons.includes("org") && <li>{t("groupMappings.grant.reasonOrg")}</li>}
        </ul>
      </ConfirmDialog>

      <ConfirmDialog
        name={`${kind}-group-mapping-remove`}
        open={!!removeTarget}
        onOpenChange={(open) => !open && setRemoveTarget(null)}
        title={t("groupMappings.removeTitle", { group: removeTarget?.group_name })}
        description={
          removeTarget
            ? removeBody(
                roleLabel(t, removeTarget.role),
                // the scope is half of what is being withdrawn: "admin" and
                // "admin on Gateway" are very different removals
                orgScopeText(t, scope, removeTarget),
              )
            : ""
        }
        confirmLabel={t("groupMappings.remove")}
        pending={remove.isPending}
        error={remove.error}
        onConfirm={() => {
          if (!removeTarget) return;
          const what = removeTarget.group_name;
          remove.mutate(removeTarget.id, {
            onSuccess: () => {
              setRemoveTarget(null);
              toast.push({ tone: "success", title: t("toast.deleted", { what }) });
            },
          });
        }}
      />
    </div>
  );
}
