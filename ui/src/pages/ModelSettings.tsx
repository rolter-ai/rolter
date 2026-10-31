import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { SettingsBody } from "@/components/screen";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";
import { Link } from "react-router";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Field } from "@/components/ui/field";
import { Combobox } from "@/components/ui/combobox";
import { Input } from "@/components/ui/input";
import { SettingsSaveBar } from "@/components/SettingsSaveBar";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { Switch } from "@/components/ui/switch";
import {
  fetchModelDefaults,
  fetchModels,
  updateModelDefaults,
  type ModelDefaultsDto,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useToast } from "@/lib/toast";
import { translateErrors } from "@/lib/use-field-errors";
import { useSettingsForm } from "@/lib/use-settings-form";
import { useScreenReady } from "@/lib/ux-react";

// every field is optional, so the form keeps raw strings and an empty string
// means "leave this to the provider" rather than "send zero"
interface FormState {
  enabled: boolean;
  defaultModel: string;
  temperature: string;
  topP: string;
  maxTokens: string;
}

const text = (value: string | null) => value ?? "";
const num = (value: number | null) => (value === null ? "" : String(value));

const fromDto = (dto: ModelDefaultsDto): FormState => ({
  enabled: dto.enabled,
  defaultModel: text(dto.default_model),
  temperature: num(dto.default_temperature),
  topP: num(dto.default_top_p),
  maxTokens: num(dto.default_max_tokens),
});

const blank = (value: string) => value.trim() === "";
const parse = (value: string) => (blank(value) ? null : Number(value));

const inRange = (value: string, min: number, max: number, integer = false) => {
  if (blank(value)) return true;
  const n = Number(value);
  if (!Number.isFinite(n)) return false;
  if (integer && !Number.isInteger(n)) return false;
  return n >= min && n <= max;
};

const validModel = (value: string) => value.length <= 256;
const validTemperature = (value: string) => inRange(value, 0, 2);
const validTopP = (value: string) => inRange(value, 0, 1);
const validMaxTokens = (value: string) => inRange(value, 1, 1_000_000, true);

// mirrors the server's validation so a bad value is caught before the round
// trip; the server stays the authority and its message is surfaced on reject.
// it names a catalog key rather than carrying english copy — the screen renders
// it, which is where `t` lives
// every failing field is reported at once, keyed by field (#2651)
type FieldKey = Exclude<keyof FormState, "enabled">;
type FieldErrors = Partial<Record<FieldKey, string>>;

// the order the fields sit in, so focus lands on the first one that is wrong
const FIELD_ORDER: FieldKey[] = ["temperature", "topP", "maxTokens", "defaultModel"];

// the wire names a 400 opens with, mapped to the field they belong to
const WIRE_FIELDS: Record<string, FieldKey> = {
  default_model: "defaultModel",
  default_temperature: "temperature",
  default_top_p: "topP",
  default_max_tokens: "maxTokens",
};

function validate(form: FormState): FieldErrors {
  const errors: FieldErrors = {};
  // every field is disabled while the defaults are off, so a bad value there
  // could not be fixed; they are re-checked once the switch is back on (#2645)
  if (!form.enabled) return errors;
  if (!validModel(form.defaultModel)) {
    errors.defaultModel = "pages.modelSettings.validation.defaultModel";
  }
  if (!validTemperature(form.temperature)) {
    errors.temperature = "pages.modelSettings.validation.temperature";
  }
  if (!validTopP(form.topP)) errors.topP = "pages.modelSettings.validation.topP";
  if (!validMaxTokens(form.maxTokens)) {
    errors.maxTokens = "pages.modelSettings.validation.maxTokens";
  }
  return errors;
}

const hasAnyDefault = (form: FormState) =>
  !blank(form.defaultModel) ||
  !blank(form.temperature) ||
  !blank(form.topP) ||
  !blank(form.maxTokens);

// deployment-wide inference defaults, persisted via /api/v1/model-defaults
// (superadmin only). they only ever fill a gap: a parameter the client sent is
// never overwritten, which is what makes this safe to turn on mid-flight
function ModelSettingsScreen() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const defaults = useQuery({
    queryKey: ["model-defaults"],
    queryFn: fetchModelDefaults,
    retry: false,
  });

  // the routes the gateway serves, for the default-model picker; a failure
  // here must not block the other settings, so it degrades to typing
  const models = useQuery({ queryKey: ["models"], queryFn: fetchModels });
  const modelOptions = React.useMemo(
    () => (models.data ?? []).map((m) => ({ value: m.model, label: m.model })),
    [models.data],
  );

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `defaults` is the query the user is actually waiting on for this screen
  useScreenReady(!defaults.isLoading);

  const base = React.useId();
  const ids: Record<FieldKey, string> = {
    temperature: `${base}-temperature`,
    topP: `${base}-top-p`,
    maxTokens: `${base}-max-tokens`,
    defaultModel: `${base}-model`,
  };
  const source = React.useMemo(
    () => (defaults.data ? fromDto(defaults.data) : undefined),
    [defaults.data],
  );
  const sf = useSettingsForm<FormState, FieldKey>({
    source,
    fieldErrors: (draft) => ({
      order: FIELD_ORDER,
      wire: WIRE_FIELDS,
      idOf: (key) => ids[key],
      // every field is disabled while the defaults are off, so none can be fixed
      skip: () => draft !== null && !draft.enabled,
      local: draft ? translateErrors(validate(draft), t) : {},
    }),
  });
  const { draft: form, fields } = sf;

  const save = useMutation({
    mutationFn: (f: FormState) => {
      // an unusable value is only reachable with the defaults off; keep what
      // is stored rather than sending a draft the server would refuse
      const stored = defaults.data;
      return updateModelDefaults({
        enabled: f.enabled,
        default_model: !validModel(f.defaultModel)
          ? (stored?.default_model ?? null)
          : blank(f.defaultModel)
            ? null
            : f.defaultModel.trim(),
        default_temperature: validTemperature(f.temperature)
          ? parse(f.temperature)
          : (stored?.default_temperature ?? null),
        default_top_p: validTopP(f.topP) ? parse(f.topP) : (stored?.default_top_p ?? null),
        default_max_tokens: validMaxTokens(f.maxTokens)
          ? parse(f.maxTokens)
          : (stored?.default_max_tokens ?? null),
      });
    },
    onSuccess: (dto) => {
      queryClient.setQueryData(["model-defaults"], dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: ["model-defaults"] });
      sf.commit(fromDto(dto));
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.modelSettings") }),
      });
    },
    onError: (error) => {
      if (fields.refuse(error)) return;
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.modelSettings") }),
        detail: errorText(error, t),
      });
    },
  });

  if (defaults.isLoading) {
    return (
      <SettingsBody>
        <PanelSkeleton panels={2} height={152} />
      </SettingsBody>
    );
  }
  if (defaults.isError) {
    return (
      <SettingsBody>
        <LoadError
          error={defaults.error}
          resource={t("errors.resources.modelSettings")}
          onRetry={() => void defaults.refetch()}
          target="model-settings"
        />
      </SettingsBody>
    );
  }
  if (!form) return null;

  // an edit answers the server's complaint about that field
  const { set, changed } = sf;
  const edit = (key: FieldKey, value: string) => set({ [key]: value });
  const active = form.enabled && hasAnyDefault(form);

  return (
    <SettingsBody onSubmit={() => fields.submit(() => save.mutate(form))}>
      <SettingsPanel
        title={t("pages.modelSettings.applyDefaults")}
        unsaved={changed.includes("enabled")}
        description={t("pages.modelSettings.applyDefaultsDesc")}
        badge={
          <Badge tone={active ? "success" : "neutral"}>
            {active ? t("pages.modelSettings.active") : t("pages.modelSettings.inactive")}
          </Badge>
        }
        action={
          <Switch
            checked={form.enabled}
            aria-label={t("pages.modelSettings.applyDefaults")}
            onCheckedChange={(v) => set({ enabled: v })}
          />
        }
      >
        {form.enabled && !hasAnyDefault(form) && (
          <p className="text-xs text-[color:var(--text-subtle)]">
            {t("pages.modelSettings.noDefaults")}
          </p>
        )}
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.modelSettings.sampling.title")}
        description={t("pages.modelSettings.sampling.desc")}
        dimmed={!form.enabled}
      >
        <Field
          label={t("pages.modelSettings.sampling.temperature")}
          hint="0 – 2"
          error={fields.errorOf("temperature")}
        >
          <Input
            id={ids.temperature}
            className="max-w-[160px]"
            placeholder={t("pages.modelSettings.providerDefault")}
            value={form.temperature}
            disabled={!form.enabled}
            onChange={(e) => edit("temperature", e.target.value)}
          />
        </Field>
        <Field
          label={t("pages.modelSettings.sampling.topP")}
          hint="0 – 1"
          error={fields.errorOf("topP")}
        >
          <Input
            id={ids.topP}
            className="max-w-[160px]"
            placeholder={t("pages.modelSettings.providerDefault")}
            value={form.topP}
            disabled={!form.enabled}
            onChange={(e) => edit("topP", e.target.value)}
          />
        </Field>
        <Field
          label={t("pages.modelSettings.sampling.maxTokens")}
          hint={t("pages.modelSettings.sampling.maxTokensHint")}
          error={fields.errorOf("maxTokens")}
        >
          <Input
            id={ids.maxTokens}
            className="max-w-[160px]"
            placeholder={t("pages.modelSettings.providerDefault")}
            value={form.maxTokens}
            disabled={!form.enabled}
            onChange={(e) => edit("maxTokens", e.target.value)}
          />
        </Field>
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.modelSettings.model.title")}
        description={t("pages.modelSettings.model.desc")}
        dimmed={!form.enabled}
      >
        <Field
          label={t("pages.modelSettings.model.defaultModel")}
          hint={
            models.isError
              ? t("pages.modelSettings.model.modelsUnavailable")
              : t("pages.modelSettings.model.defaultModelHint")
          }
          error={fields.errorOf("defaultModel")}
        >
          <Combobox
            id={ids.defaultModel}
            className="sm:min-w-[320px]"
            options={modelOptions}
            allowCustom
            // typed text is the value as it is typed, so clearing and retyping
            // a name is one edit; a clear alone is not committed on blur (#3244)
            commitOnInput
            clearable
            placeholder={t("pages.modelSettings.model.placeholder")}
            value={form.defaultModel}
            disabled={!form.enabled}
            onChange={(v) => edit("defaultModel", v)}
          />
        </Field>
      </SettingsPanel>

      <p className="text-xs text-muted-foreground">
        <Trans
          i18nKey="pages.modelSettings.timeoutsNote"
          components={[
            <Link
              key="where"
              to="/performance"
              className="rounded-sm text-[color:var(--text-secondary)] underline decoration-[color:var(--border-strong)] underline-offset-4 transition-colors hover:decoration-current focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            />,
          ]}
        />
      </p>
      <p className="text-xs text-muted-foreground">{t("pages.modelSettings.precedenceNote")}</p>

      <SettingsSaveBar form={sf} pending={save.isPending} updatedAt={defaults.data?.updated_at} />
    </SettingsBody>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(
  ModelSettingsScreen,
  "errors.resources.modelSettings",
  "model_defaults:read",
);
