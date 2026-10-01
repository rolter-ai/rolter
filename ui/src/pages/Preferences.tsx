import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { PageBody } from "@/components/screen";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import { Field } from "@/components/ui/field";
import { FieldError, describedBy } from "@/components/ui/field-error";
import { FieldLabel } from "@/components/ui/field-label";
import { Input } from "@/components/ui/input";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { Segmented } from "@/components/ui/segmented";
import {
  fetchOrgs,
  fetchProjects,
  fetchTeams,
  fetchPreferences,
  preferencesDocument,
  type UserPreferences,
} from "@/lib/api";
import { LOCALES, LOCALE_NAMES, type Locale } from "@/lib/i18n";
import { validTimeZone } from "@/lib/i18n/format";
import { PREFERENCES_QUERY_KEY, refusedField, savePreferences } from "@/lib/preferences";
import { errorDetail, useToast } from "@/lib/toast";
import { useDraft } from "@/lib/use-draft";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

// the account's own preferences (#2448), stored on the control plane so they
// follow the account to another browser. what a save sends is the document as
// the server last held it with this form's edits laid over it, never the form
// alone: PUT replaces the whole document

const BROWSER = "";

// the key a save names; `effective_default_scope` is computed on the server
// and refused if sent back, so it never enters the draft
type PreferenceKey = keyof UserPreferences;

/** every zone this engine knows, plus UTC, which older engines leave out of the list */
function knownTimeZones(): string[] {
  const list = (Intl as unknown as { supportedValuesOf?: (key: string) => string[] })
    .supportedValuesOf;
  const zones = list ? list("timeZone") : [];
  return zones.includes("UTC") ? zones : ["UTC", ...zones];
}

function browserTimeZone(): string {
  return new Intl.DateTimeFormat().resolvedOptions().timeZone;
}

export default function Preferences() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();

  const prefs = useQuery({
    queryKey: PREFERENCES_QUERY_KEY,
    queryFn: fetchPreferences,
    retry: false,
  });
  useScreenReady(!prefs.isLoading);
  useErrorState(!!prefs.error, "preferences");

  // a copy cached for first paint is stale by construction (it is a guess made
  // before the fetch answered), so the form waits for the server's word
  const answered = prefs.dataUpdatedAt > 0 && prefs.data ? prefs.data : undefined;
  const source = React.useMemo(
    () => (answered ? preferencesDocument(answered) : undefined),
    [answered],
  );
  const { draft, changed, dirty, set, reset, commit } = useDraft<UserPreferences>(source);

  const org = draft?.default_org_id ?? "";
  const team = draft?.default_team_id ?? "";
  const orgs = useQuery({ queryKey: ["scope", "orgs"], queryFn: fetchOrgs, enabled: !!draft });
  const teams = useQuery({
    queryKey: ["scope", "teams", org],
    queryFn: () => fetchTeams(org),
    enabled: !!org,
  });
  const projects = useQuery({
    queryKey: ["scope", "projects", team],
    queryFn: () => fetchProjects(team),
    enabled: !!team,
  });

  const save = useMutation({
    mutationFn: (patch: Partial<UserPreferences>) => savePreferences(queryClient, patch),
    onSuccess: (saved) => {
      commit(preferencesDocument(saved));
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("screens.preferences.title") }),
      });
    },
  });

  const zones = React.useMemo(knownTimeZones, []);
  const zone = draft?.chart_time_zone ?? "";
  const zoneOptions = React.useMemo(
    () => (zone && !zones.includes(zone) ? [zone, ...zones] : zones),
    [zones, zone],
  );

  if (prefs.isError) {
    return (
      <PageBody>
        <LoadError
          error={prefs.error}
          resource={t("errors.resources.preferences")}
          onRetry={() => void prefs.refetch()}
        />
      </PageBody>
    );
  }
  if (!draft || !answered) {
    return (
      <PageBody className="mx-auto w-full max-w-[840px]">
        <PanelSkeleton panels={3} height={148} />
      </PageBody>
    );
  }

  // a 400 names the field it refused; one that names none is said once, by the
  // save button, so it is never lost
  const refused = save.error ? refusedField(save.error) : null;
  const refusal = save.error ? errorDetail(save.error) : undefined;
  const fieldError = (field: PreferenceKey): string | undefined =>
    refused === field ? refusal : undefined;
  const formError = save.error && !refused ? refusal : undefined;
  // an edit clears the refusal it answers
  const edit = (patch: Partial<UserPreferences>) => {
    if (save.isError) save.reset();
    set(patch);
  };

  const effective = answered.effective_default_scope;
  const storedScope = !!(
    answered.default_org_id ||
    answered.default_team_id ||
    answered.default_project_id
  );
  // a default the account can no longer read: the server computed a different
  // answer (or none), and the dashboard follows that
  const scopeLost =
    storedScope &&
    (effective === null ||
      effective.org_id !== answered.default_org_id ||
      (answered.default_team_id !== null && effective.team_id !== answered.default_team_id) ||
      (answered.default_project_id !== null &&
        effective.project_id !== answered.default_project_id));

  const language = draft.language ?? BROWSER;
  const languageId = "preferences-language-label";
  const modelId = "preferences-model";
  const modelErrorId = "preferences-model-error";
  const languageErrorId = "preferences-language-error";
  const zoneUnknown = zone !== "" && validTimeZone(zone) === undefined;
  const zoneHint = zoneUnknown
    ? t("pages.preferences.zone.unknown", { zone })
    : t("pages.preferences.zone.hint", { zone: browserTimeZone() });

  const options = (rows: { id: string; name: string }[] | undefined) =>
    (rows ?? []).map((row) => ({ value: row.id, label: row.name }));

  return (
    <PageBody className="mx-auto w-full max-w-[840px]">
      <SettingsPanel
        title={t("pages.preferences.language.title")}
        description={t("pages.preferences.language.hint")}
      >
        <div className="flex flex-col gap-1.5">
          <FieldLabel label={t("pages.preferences.language.label")} id={languageId} />
          <Segmented<string>
            labelledBy={languageId}
            value={language}
            onChange={(next) => edit({ language: next === BROWSER ? null : next })}
            options={[
              { value: BROWSER, label: t("pages.preferences.language.browser") },
              ...LOCALES.map((locale: Locale) => ({ value: locale, label: LOCALE_NAMES[locale] })),
            ]}
          />
          <FieldError id={languageErrorId} error={fieldError("language")} />
        </div>
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.preferences.scope.title")}
        description={t("pages.preferences.scope.hint")}
      >
        <div className="grid w-full gap-3 sm:grid-cols-3">
          <Field label={t("scope.org")}>
            <Combobox
              clearable
              value={org}
              placeholder={t("pages.preferences.scope.none")}
              options={options(orgs.data)}
              onChange={(next) =>
                edit({
                  default_org_id: next || null,
                  default_team_id: null,
                  default_project_id: null,
                })
              }
            />
          </Field>
          <Field label={t("scope.team")}>
            <Combobox
              clearable
              value={team}
              disabled={!org}
              placeholder={t("pages.preferences.scope.none")}
              options={options(teams.data)}
              onChange={(next) => edit({ default_team_id: next || null, default_project_id: null })}
            />
          </Field>
          <Field label={t("scope.project")}>
            <Combobox
              clearable
              value={draft.default_project_id ?? ""}
              disabled={!team}
              placeholder={t("pages.preferences.scope.none")}
              options={options(projects.data)}
              onChange={(next) => edit({ default_project_id: next || null })}
            />
          </Field>
        </div>
        {scopeLost && (
          <p role="status" className="w-full text-xs text-[color:var(--status-warning-text)]">
            {effective === null
              ? t("pages.preferences.scope.lost")
              : t("pages.preferences.scope.moved")}
          </p>
        )}
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.preferences.playground.title")}
        description={t("pages.preferences.playground.hint")}
      >
        <div className="flex w-full flex-col gap-1.5">
          <FieldLabel label={t("pages.preferences.playground.label")} htmlFor={modelId} />
          <Input
            id={modelId}
            className="max-w-[360px] font-mono text-xs"
            placeholder={t("pages.preferences.playground.placeholder")}
            value={draft.default_playground_model ?? ""}
            aria-invalid={!!fieldError("default_playground_model")}
            aria-describedby={describedBy(!!fieldError("default_playground_model") && modelErrorId)}
            onChange={(e) => edit({ default_playground_model: e.target.value || null })}
          />
          <FieldError id={modelErrorId} error={fieldError("default_playground_model")} />
        </div>
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.preferences.zone.title")}
        description={t("pages.preferences.zone.description")}
      >
        <div className="w-full max-w-[360px]">
          <Field
            label={t("pages.preferences.zone.label")}
            hint={fieldError("chart_time_zone") ? undefined : zoneHint}
            error={fieldError("chart_time_zone")}
          >
            <Combobox
              clearable
              value={zone}
              placeholder={t("pages.preferences.zone.browser", { zone: browserTimeZone() })}
              options={zoneOptions.map((name) => ({ value: name, label: name }))}
              onChange={(next) => edit({ chart_time_zone: next || null })}
            />
          </Field>
        </div>
      </SettingsPanel>

      <div className="sticky bottom-0 flex items-center justify-end gap-3 border-t border-[color:var(--border-subtle)] bg-background py-3">
        {formError && (
          <span role="alert" className="text-xs text-[color:var(--status-danger-text)]">
            {formError}
          </span>
        )}
        <Button variant="outline" disabled={!dirty || save.isPending} onClick={reset}>
          {t("pages.preferences.discard")}
        </Button>
        <Button
          disabled={!dirty || save.isPending}
          onClick={() => {
            // only the edited keys go over the latest document, so a key set
            // elsewhere since this form loaded is not wiped by a save
            const patch: Partial<UserPreferences> = {};
            for (const key of changed) patch[key] = draft[key] as never;
            save.mutate(patch);
          }}
        >
          {save.isPending ? t("common.saving") : t("common.saveChanges")}
        </Button>
      </div>
    </PageBody>
  );
}
