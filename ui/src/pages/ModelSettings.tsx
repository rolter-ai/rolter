import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  fetchModelDefaults,
  updateModelDefaults,
  type ModelDefaultsDto,
} from "@/lib/api";
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

// mirrors the server's validation so a bad value is caught before the round
// trip; the server stays the authority and its message is surfaced on reject.
// it names a catalog key rather than carrying english copy — the screen renders
// it, which is where `t` lives
function validate(form: FormState): string | null {
  if (form.defaultModel.length > 256) {
    return "pages.modelSettings.validation.defaultModel";
  }
  if (!inRange(form.temperature, 0, 2)) return "pages.modelSettings.validation.temperature";
  if (!inRange(form.topP, 0, 1)) return "pages.modelSettings.validation.topP";
  if (!inRange(form.maxTokens, 1, 1_000_000, true)) {
    return "pages.modelSettings.validation.maxTokens";
  }
  return null;
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
  React.useEffect(() => {
    if (defaults.data && form === null) {
      setForm(fromDto(defaults.data));
    }
  }, [defaults.data, form]);

  const save = useMutation({
    mutationFn: (f: FormState) =>
      updateModelDefaults({
        enabled: f.enabled,
        default_model: blank(f.defaultModel) ? null : f.defaultModel.trim(),
        default_temperature: parse(f.temperature),
        default_top_p: parse(f.topP),
        default_max_tokens: parse(f.maxTokens),
      }),
    onSuccess: (dto) => {
      queryClient.setQueryData(["model-defaults"], dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: ["model-defaults"] });
      setForm(fromDto(dto));
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.modelSettings") }),
      });
    },
    onError: (error) => {
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
  const localErrorKey = validate(form);
  const localError = localErrorKey ? t(localErrorKey) : null;
  const active = form.enabled && hasAnyDefault(form);

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div className="flex items-start gap-4">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="text-sm font-medium">
                {t("pages.modelSettings.applyDefaults")}
              </span>
              <Badge tone={active ? "success" : "neutral"} className="font-mono text-[10px] uppercase">
                {active
                  ? t("pages.modelSettings.active")
                  : t("pages.modelSettings.inactive")}
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

      <Card
        title={t("pages.modelSettings.sampling.title")}
        desc={t("pages.modelSettings.sampling.desc")}
        dimmed={!form.enabled}
      >
        <Field
          label={t("pages.modelSettings.sampling.temperature")}
          hint="0 – 2"
          value={form.temperature}
          disabled={!form.enabled}
          onChange={(v) => set({ temperature: v })}
        />
        <Field
          label={t("pages.modelSettings.sampling.topP")}
          hint="0 – 1"
          value={form.topP}
          disabled={!form.enabled}
          onChange={(v) => set({ topP: v })}
        />
        <Field
          label={t("pages.modelSettings.sampling.maxTokens")}
          hint={t("pages.modelSettings.sampling.maxTokensHint")}
          value={form.maxTokens}
          disabled={!form.enabled}
          onChange={(v) => set({ maxTokens: v })}
        />
      </Card>

      <Card
        title={t("pages.modelSettings.model.title")}
        desc={t("pages.modelSettings.model.desc")}
        dimmed={!form.enabled}
      >
        <Field
          label={t("pages.modelSettings.model.defaultModel")}
          hint={t("pages.modelSettings.model.defaultModelHint")}
          wide
          value={form.defaultModel}
          disabled={!form.enabled}
          onChange={(v) => set({ defaultModel: v })}
        />
      </Card>

      <p className="text-xs text-muted-foreground">
        <Trans
          i18nKey="pages.modelSettings.timeoutsNote"
          components={[<span key="where" className="text-[color:var(--text-secondary)]" />]}
        />
      </p>

      <div className="sticky bottom-0 flex items-center justify-end gap-3 border-t border-[color:var(--border-subtle)] bg-background py-3">
        {localError && <span className="text-xs text-[color:var(--status-danger-text)]">{localError}</span>}
        <Button disabled={save.isPending || localError !== null} onClick={() => save.mutate(form)}>
          {save.isPending ? t("common.saving") : t("common.saveChanges")}
        </Button>
      </div>
    </div>
  );
}

function Card({
  title,
  desc,
  dimmed = false,
  children,
}: {
  title: string;
  desc: string;
  dimmed?: boolean;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
      <div>
        <span className="text-sm font-medium">{title}</span>
        <p className="mt-1 text-sm text-muted-foreground">{desc}</p>
      </div>
      {/* a disabled fieldset rather than a dimmed div: every control inside
          already carries `disabled`, and fading a live div drags its labels and
          hints below 4.5:1 while telling assistive tech nothing (#1181) */}
      <fieldset className="flex min-w-0 flex-wrap gap-4" disabled={dimmed} style={{ opacity: dimmed ? 0.55 : 1 }}>
        {children}
      </fieldset>
    </section>
  );
}

function Field({
  label,
  hint,
  value,
  disabled = false,
  wide = false,
  onChange,
}: {
  label: string;
  hint: string;
  value: string;
  disabled?: boolean;
  wide?: boolean;
  onChange: (v: string) => void;
}) {
  const { t } = useTranslation();
  const id = React.useId();
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={id} className="text-xs font-medium text-[color:var(--text-secondary)]">
        {label}
      </label>
      <Input
        id={id}
        className={wide ? "min-w-[320px]" : "max-w-[160px]"}
        placeholder={t("pages.modelSettings.providerDefault")}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
      />
      <span className="text-[0.6875rem] text-[color:var(--text-subtle)]">{hint}</span>
    </div>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(ModelSettingsScreen, "errors.resources.modelSettings");
