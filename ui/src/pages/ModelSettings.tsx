import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { Switch } from "@/components/ui/switch";
import { fetchModelDefaults, updateModelDefaults, type ModelDefaultsDto } from "@/lib/api";
import { serverFieldError } from "@/lib/field-errors";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

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

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `defaults` is the query the user is actually waiting on for this screen
  useScreenReady(!defaults.isLoading);
  useErrorState(!!defaults.error, "model-settings");

  const [form, setForm] = React.useState<FormState | null>(null);
  const [serverErrors, setServerErrors] = React.useState<FieldErrors>({});
  const base = React.useId();
  const ids: Record<FieldKey, string> = {
    temperature: `${base}-temperature`,
    topP: `${base}-top-p`,
    maxTokens: `${base}-max-tokens`,
    defaultModel: `${base}-model`,
  };
  React.useEffect(() => {
    if (defaults.data && form === null) {
      setForm(fromDto(defaults.data));
    }
  }, [defaults.data, form]);

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
      setForm(fromDto(dto));
      setServerErrors({});
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.modelSettings") }),
      });
    },
    onError: (error) => {
      const named = serverFieldError(error, WIRE_FIELDS);
      if (named) {
        setServerErrors({ [named.field]: named.message });
        document.getElementById(ids[named.field])?.focus();
        return;
      }
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.modelSettings") }),
        detail: errorDetail(error),
      });
    },
  });

  if (defaults.isLoading) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <PanelSkeleton panels={2} height={152} />
      </div>
    );
  }
  if (defaults.isError) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <LoadError
          error={defaults.error}
          resource={t("errors.resources.modelSettings")}
          onRetry={() => void defaults.refetch()}
        />
      </div>
    );
  }
  if (!form) return null;

  const set = (patch: Partial<FormState>) => {
    setForm((f) => (f ? { ...f, ...patch } : f));
  };
  // an edit answers the server's complaint about that field
  const edit = (key: FieldKey, value: string) => {
    set({ [key]: value });
    setServerErrors((e) => ({ ...e, [key]: undefined }));
  };
  const local = validate(form);
  const errorFor = (key: FieldKey) => {
    // a disabled field cannot be fixed, so no complaint about it holds Save
    if (!form.enabled) return undefined;
    const localKey = local[key];
    return localKey ? t(localKey) : serverErrors[key];
  };
  const errors = Object.fromEntries(FIELD_ORDER.map((key) => [key, errorFor(key)])) as Record<
    FieldKey,
    string | undefined
  >;
  const invalid = FIELD_ORDER.filter((key) => errors[key]);
  // Save stays pressable while the form is invalid so a press can say why: it
  // moves focus to the first field at fault rather than doing nothing (#2651)
  const submit = () => {
    if (invalid.length > 0) {
      document.getElementById(ids[invalid[0]])?.focus();
      return;
    }
    save.mutate(form);
  };
  const active = form.enabled && hasAnyDefault(form);

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div className="flex items-start gap-4">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="text-sm font-medium">{t("pages.modelSettings.applyDefaults")}</span>
              <Badge
                tone={active ? "success" : "neutral"}
                className="font-mono text-[10px] uppercase"
              >
                {active ? t("pages.modelSettings.active") : t("pages.modelSettings.inactive")}
              </Badge>
            </div>
            <p className="mt-1 text-sm text-muted-foreground">
              {t("pages.modelSettings.applyDefaultsDesc")}
            </p>
          </div>
          <Switch
            checked={form.enabled}
            aria-label={t("pages.modelSettings.applyDefaults")}
            onCheckedChange={(v) => set({ enabled: v })}
          />
        </div>
        {form.enabled && !hasAnyDefault(form) && (
          <p className="text-xs text-[color:var(--text-subtle)]">
            {t("pages.modelSettings.noDefaults")}
          </p>
        )}
      </section>

      <SettingsPanel
        title={t("pages.modelSettings.sampling.title")}
        description={t("pages.modelSettings.sampling.desc")}
        dimmed={!form.enabled}
      >
        <Field
          label={t("pages.modelSettings.sampling.temperature")}
          hint="0 – 2"
          error={errors.temperature}
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
        <Field label={t("pages.modelSettings.sampling.topP")} hint="0 – 1" error={errors.topP}>
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
          error={errors.maxTokens}
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
          hint={t("pages.modelSettings.model.defaultModelHint")}
          error={errors.defaultModel}
        >
          <Input
            id={ids.defaultModel}
            className="min-w-[320px]"
            placeholder={t("pages.modelSettings.providerDefault")}
            value={form.defaultModel}
            disabled={!form.enabled}
            onChange={(e) => edit("defaultModel", e.target.value)}
          />
        </Field>
      </SettingsPanel>

      <p className="text-xs text-muted-foreground">
        <Trans
          i18nKey="pages.modelSettings.timeoutsNote"
          components={[<span key="where" className="text-[color:var(--text-secondary)]" />]}
        />
      </p>

      <div className="sticky bottom-0 flex items-center justify-end gap-3 border-t border-[color:var(--border-subtle)] bg-background py-3">
        {invalid.length > 0 && (
          <span role="status" className="text-xs text-[color:var(--status-danger-text)]">
            {t("common.fieldsNeedAttention", { count: invalid.length })}
          </span>
        )}
        <Button
          disabled={save.isPending}
          aria-disabled={invalid.length > 0 || undefined}
          className={invalid.length > 0 ? "opacity-50" : undefined}
          onClick={submit}
        >
          {save.isPending ? t("common.saving") : t("common.saveChanges")}
        </Button>
      </div>
    </div>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(ModelSettingsScreen, "errors.resources.modelSettings");
