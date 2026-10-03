import { useTranslation } from "react-i18next";

import { GatedCombobox } from "@/components/GatedCombobox";
import { LoadError } from "@/components/LoadError";
import { ControlSkeleton } from "@/components/LoadingState";
import { ORG_TARGET, orgScopeText, useOrgScope, type OrgScope } from "@/components/OrgScopePicker";
import { Badge } from "@/components/ui/badge";
import { Combobox, type ComboboxOption } from "@/components/ui/combobox";
import { Field } from "@/components/ui/field";
import { useCan } from "@/lib/can";

/**
 * The scope row of the provider and provider-group sheets (#1919).
 *
 * `""` is the whole organization, the default and what every existing row is;
 * anything else is the id of a project of the org, and then only keys minted in
 * that project reach the row. The two resources share one picker because they
 * share one rule, and one set of copy.
 *
 * Who may do what mirrors the control plane, which stays the authority: making
 * a row org-wide, or moving it, is an org admin's call, and a project admin may
 * only create a row for a project of their own. `provider` and `provider_group`
 * are org-scoped capabilities, so the effective answer for the current scope is
 * "no" for exactly the callers the server refuses.
 */
export type ScopedResource = "provider" | "provider_group";

/**
 * Whether the caller may leave a row org-wide, or change the scope of one.
 *
 * An unanswered question reads as yes, like every other gate: the server
 * refuses what the dashboard let through, and an old control plane that cannot
 * answer must not strip the picker of its org-wide option.
 */
export function useMayWiden(resource: ScopedResource, mode: "add" | "edit"): boolean {
  const can = useCan();
  return can(resource, mode === "add" ? "create" : "update") !== false;
}

export function ProjectScopeField({
  resource,
  mode,
  orgId,
  value,
  onChange,
  mayWiden,
  id,
}: {
  resource: ScopedResource;
  mode: "add" | "edit";
  orgId: string | null;
  /** `""` is the whole organization */
  value: string;
  onChange: (value: string) => void;
  /** from `useMayWiden`, which the sheet also needs for `api_key_env` */
  mayWiden: boolean;
  id: string;
}) {
  const { t } = useTranslation();
  const scope = useOrgScope(orgId ?? undefined);
  const locked = mode === "edit" && !mayWiden;

  const options: ComboboxOption[] = [
    ...(mayWiden ? [{ value: ORG_TARGET, label: t("scope.picker.org") }] : []),
    ...scope.byTeam.flatMap((entry) =>
      entry.projects.map((project) => ({
        value: project.id,
        label: project.name,
        group: t("scope.picker.teamProjects", { team: entry.team.name }),
      })),
    ),
  ];
  // a scope the list does not name (a project the caller cannot read) is still
  // the value, so the picker shows it rather than a blank that reads as "none"
  if (value && !options.some((o) => o.value === value)) {
    options.push({ value, label: orgScopeText(t, scope, { project_id: value }) });
  }

  if (scope.isLoading) {
    return (
      <Field label={t("projectScope.label")} htmlFor={id}>
        <ControlSkeleton width="100%" />
      </Field>
    );
  }

  const shared = { id, value, onChange, options };
  const hint = locked
    ? t("projectScope.hintLocked")
    : mayWiden
      ? t("projectScope.hint")
      : t("projectScope.hintOwnProject");

  return (
    <Field label={t("projectScope.label")} htmlFor={id} hint={hint}>
      {mode === "add" ? (
        <Combobox {...shared} />
      ) : resource === "provider" ? (
        <GatedCombobox gate="provider:update" control="provider-scope" {...shared} />
      ) : (
        <GatedCombobox gate="provider_group:update" control="provider-group-scope" {...shared} />
      )}
      {!!scope.error && (
        <LoadError
          error={scope.error}
          resource={t("errors.resources.orgScope")}
          onRetry={scope.refetch}
        />
      )}
    </Field>
  );
}

/**
 * Where a row is scoped, for the lists: "Project: Gateway", or the plain
 * word for an org-wide one.
 *
 * Takes the org's scope already read, so a list pays for one request rather
 * than one per row.
 */
export function ProjectScopeBadge({
  projectId,
  scope,
}: {
  projectId?: string | null;
  scope: OrgScope;
}) {
  const { t } = useTranslation();
  if (!projectId) {
    return <span className="text-xs text-muted-foreground">{t("projectScope.orgWide")}</span>;
  }
  return (
    <Badge tone="outline" className="max-w-full truncate">
      {t("projectScope.badge", {
        project: orgScopeText(t, scope, { project_id: projectId }),
      })}
    </Badge>
  );
}
