import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { Switch } from "@/components/ui/switch";
import {
  fetchRuntimePolicy,
  updateRuntimePolicy,
  BACKPRESSURE_POLICIES,
  type BackpressurePolicy,
  type RuntimePolicyDto,
} from "@/lib/api";
import { serverFieldError } from "@/lib/field-errors";
import { errorDetail, useToast } from "@/lib/toast";
import { useScreenReady } from "@/lib/ux-react";

interface FormState {
  retryMaxRetries: string;
  retryBaseMs: string;
  retryMaxMs: string;
  timeoutConnectS: string;
  timeoutRequestS: string;
  queueEnabled: boolean;
  queueCapacity: string;
  queueWorkers: string;
  queueBackpressure: BackpressurePolicy;
  queueBlockMs: string;
}

const fromDto = (dto: RuntimePolicyDto): FormState => ({
  retryMaxRetries: String(dto.retry_max_retries),
  retryBaseMs: String(dto.retry_base_ms),
  retryMaxMs: String(dto.retry_max_ms),
  timeoutConnectS: String(dto.timeout_connect_s),
  timeoutRequestS: String(dto.timeout_request_s),
  queueEnabled: dto.queue_enabled,
  queueCapacity: String(dto.queue_capacity),
  queueWorkers: String(dto.queue_workers),
  queueBackpressure: dto.queue_backpressure,
  queueBlockMs: String(dto.queue_block_ms),
});

// the catalog key describing each policy; the copy itself lives in en.json
const BACKPRESSURE_COPY: Record<BackpressurePolicy, string> = {
  drop: "pages.performance.backpressure.drop",
  block: "pages.performance.backpressure.block",
  error: "pages.performance.backpressure.error",
};

const inRange = (value: string, min: number, max: number) => {
  const n = Number(value);
  return Number.isInteger(n) && n >= min && n <= max;
};

const validCapacity = (value: string) => inRange(value, 1, 100_000);
const validWorkers = (value: string) => inRange(value, 1, 2_048);
const validBlockMs = (value: string) => inRange(value, 0, 120_000);

// mirrors the server's validation so a bad value is caught before the round
// trip; the server stays the authority and its message is surfaced on reject.
// it names a catalog key rather than carrying english copy — the screen renders
// it, which is where `t` lives
// every failing field is reported at once, keyed by field (#2651)
type FieldKey = Exclude<keyof FormState, "queueEnabled" | "queueBackpressure">;
type FieldErrors = Partial<Record<FieldKey, string>>;

// the order the fields sit in, so focus lands on the first one that is wrong
const FIELD_ORDER: FieldKey[] = [
  "retryMaxRetries",
  "retryBaseMs",
  "retryMaxMs",
  "timeoutConnectS",
  "timeoutRequestS",
  "queueCapacity",
  "queueWorkers",
  "queueBlockMs",
];

// the wire names a 400 opens with, mapped to the field they belong to
const WIRE_FIELDS: Record<string, FieldKey> = {
  retry_max_retries: "retryMaxRetries",
  retry_base_ms: "retryBaseMs",
  retry_max_ms: "retryMaxMs",
  timeout_connect_s: "timeoutConnectS",
  timeout_request_s: "timeoutRequestS",
  queue_capacity: "queueCapacity",
  queue_workers: "queueWorkers",
  queue_block_ms: "queueBlockMs",
};

// the queue fields are disabled while the queue is off, and the block timeout
// while the policy is not `block`, so a bad value there could not be fixed;
// each is re-checked once its control is enabled again (#2645)
const fieldDisabled = (form: FormState, key: FieldKey) => {
  if (key === "queueCapacity" || key === "queueWorkers") return !form.queueEnabled;
  if (key === "queueBlockMs") return !form.queueEnabled || form.queueBackpressure !== "block";
  return false;
};

function validate(form: FormState): FieldErrors {
  const key = "pages.performance.validation.";
  const errors: FieldErrors = {};
  if (!inRange(form.retryMaxRetries, 0, 10)) errors.retryMaxRetries = `${key}maxRetries`;
  const baseOk = inRange(form.retryBaseMs, 0, 60_000);
  if (!baseOk) errors.retryBaseMs = `${key}retryBase`;
  if (!inRange(form.retryMaxMs, 0, 600_000)) {
    errors.retryMaxMs = `${key}retryCap`;
  } else if (baseOk && Number(form.retryMaxMs) < Number(form.retryBaseMs)) {
    errors.retryMaxMs = `${key}retryCapBelowBase`;
  }
  if (!inRange(form.timeoutConnectS, 0, 300)) errors.timeoutConnectS = `${key}connectTimeout`;
  if (!inRange(form.timeoutRequestS, 0, 3_600)) {
    errors.timeoutRequestS = `${key}requestTimeout`;
  }
  if (!fieldDisabled(form, "queueCapacity") && !validCapacity(form.queueCapacity)) {
    errors.queueCapacity = `${key}queueCapacity`;
  }
  if (!fieldDisabled(form, "queueWorkers") && !validWorkers(form.queueWorkers)) {
    errors.queueWorkers = `${key}queueWorkers`;
  }
  if (!fieldDisabled(form, "queueBlockMs")) {
    if (!validBlockMs(form.queueBlockMs)) {
      errors.queueBlockMs = `${key}blockTimeout`;
    } else if (Number(form.queueBlockMs) === 0) {
      // blocking with a zero timeout would park callers forever
      errors.queueBlockMs = `${key}blockNeedsTimeout`;
    }
  }
  return errors;
}

// global runtime policy, persisted via /api/v1/runtime-policy (superadmin only).
// covers upstream retries, timeouts and the bounded admission queue every
// provider gets its own instance of
function PerformanceScreen() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const policy = useQuery({
    queryKey: ["runtime-policy"],
    queryFn: fetchRuntimePolicy,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `policy` is the query the user is actually waiting on for this screen
  useScreenReady(!policy.isLoading);

  const [form, setForm] = React.useState<FormState | null>(null);
  const [serverErrors, setServerErrors] = React.useState<FieldErrors>({});
  const base = React.useId();
  const idOf = (key: FieldKey) => `${base}-${key}`;
  React.useEffect(() => {
    if (policy.data && form === null) {
      setForm(fromDto(policy.data));
    }
  }, [policy.data, form]);

  const save = useMutation({
    mutationFn: (f: FormState) => {
      // an unusable queue value is only reachable while its field is
      // disabled; keep what is stored rather than sending a draft the server
      // would refuse
      const stored = policy.data;
      const keep = (draft: string, valid: boolean, fallback: number | undefined) =>
        valid ? Number(draft) : (fallback ?? Number(draft));
      return updateRuntimePolicy({
        retry_max_retries: Number(f.retryMaxRetries),
        retry_base_ms: Number(f.retryBaseMs),
        retry_max_ms: Number(f.retryMaxMs),
        timeout_connect_s: Number(f.timeoutConnectS),
        timeout_request_s: Number(f.timeoutRequestS),
        queue_enabled: f.queueEnabled,
        queue_capacity: keep(
          f.queueCapacity,
          validCapacity(f.queueCapacity),
          stored?.queue_capacity,
        ),
        queue_workers: keep(f.queueWorkers, validWorkers(f.queueWorkers), stored?.queue_workers),
        queue_backpressure: f.queueBackpressure,
        queue_block_ms: keep(f.queueBlockMs, validBlockMs(f.queueBlockMs), stored?.queue_block_ms),
      });
    },
    onSuccess: (dto) => {
      queryClient.setQueryData(["runtime-policy"], dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: ["runtime-policy"] });
      setForm(fromDto(dto));
      setServerErrors({});
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.performanceSettings") }),
      });
    },
    onError: (error) => {
      const named = serverFieldError(error, WIRE_FIELDS);
      if (named) {
        setServerErrors({ [named.field]: named.message });
        document.getElementById(idOf(named.field))?.focus();
        return;
      }
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.performanceSettings") }),
        detail: errorDetail(error),
      });
    },
  });

  if (policy.isLoading) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <PanelSkeleton panels={3} height={132} />
      </div>
    );
  }
  if (policy.isError) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <LoadError
          error={policy.error}
          resource={t("errors.resources.performanceSettings")}
          onRetry={() => void policy.refetch()}
          target="performance"
        />
      </div>
    );
  }
  if (!form) return null;

  const set = (patch: Partial<FormState>) => {
    setForm((f) => (f ? { ...f, ...patch } : f));
  };
  const local = validate(form);
  const errors = Object.fromEntries(
    FIELD_ORDER.map((key) => {
      const localKey = local[key];
      // a disabled field cannot be fixed, so no complaint about it holds Save
      const error = fieldDisabled(form, key)
        ? undefined
        : localKey
          ? t(localKey)
          : serverErrors[key];
      return [key, error];
    }),
  ) as Record<FieldKey, string | undefined>;
  const invalid = FIELD_ORDER.filter((key) => errors[key]);
  // Save stays pressable while the form is invalid so a press can say why: it
  // moves focus to the first field at fault rather than doing nothing (#2651)
  const submit = () => {
    if (invalid.length > 0) {
      document.getElementById(idOf(invalid[0]))?.focus();
      return;
    }
    save.mutate(form);
  };
  // the props every number field shares: its id, its value, its error, and an
  // edit that answers the server's complaint about that field
  const numberField = (key: FieldKey) => ({
    id: idOf(key),
    value: form[key],
    error: errors[key],
    onChange: (v: string) => {
      set({ [key]: v });
      setServerErrors((e) => ({ ...e, [key]: undefined }));
    },
  });
  const queue = form.queueEnabled;

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      <SettingsPanel
        title={t("pages.performance.retries.title")}
        description={t("pages.performance.retries.desc")}
      >
        <NumberField
          label={t("pages.performance.retries.maxRetries")}
          {...numberField("retryMaxRetries")}
        />
        <NumberField
          label={t("pages.performance.retries.baseBackoff")}
          {...numberField("retryBaseMs")}
        />
        <NumberField
          label={t("pages.performance.retries.backoffCap")}
          {...numberField("retryMaxMs")}
        />
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.performance.timeouts.title")}
        description={t("pages.performance.timeouts.desc")}
      >
        <NumberField
          label={t("pages.performance.timeouts.connect")}
          {...numberField("timeoutConnectS")}
        />
        <NumberField
          label={t("pages.performance.timeouts.request")}
          {...numberField("timeoutRequestS")}
        />
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.performance.queue.title")}
        description={t("pages.performance.queue.desc")}
        dimmed={!queue}
        action={
          <Switch
            checked={form.queueEnabled}
            aria-label={t("pages.performance.queue.toggleAria")}
            onCheckedChange={(v) => set({ queueEnabled: v })}
          />
        }
      >
        <NumberField
          label={t("pages.performance.queue.capacity")}
          {...numberField("queueCapacity")}
          disabled={!queue}
        />
        <NumberField
          label={t("pages.performance.queue.workers")}
          {...numberField("queueWorkers")}
          disabled={!queue}
        />
        <div className="flex min-w-[200px] flex-col gap-1.5">
          <label
            htmlFor="perf-queue-backpressure"
            className="text-xs font-medium text-[color:var(--text-secondary)]"
          >
            {t("pages.performance.queue.whenFull")}
          </label>
          <Combobox
            id="perf-queue-backpressure"
            value={form.queueBackpressure}
            disabled={!queue}
            aria-label={t("pages.performance.queue.whenFull")}
            onChange={(picked) => set({ queueBackpressure: picked as BackpressurePolicy })}
            options={BACKPRESSURE_POLICIES.map((p) => ({ value: p, label: p }))}
          />
          <span className="text-[0.6875rem] text-[color:var(--text-subtle)]">
            {t(BACKPRESSURE_COPY[form.queueBackpressure])}
          </span>
        </div>
        <NumberField
          label={t("pages.performance.queue.blockTimeout")}
          {...numberField("queueBlockMs")}
          disabled={!queue || form.queueBackpressure !== "block"}
        />
      </SettingsPanel>

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

function NumberField({
  id,
  label,
  value,
  error,
  disabled = false,
  onChange,
}: {
  id: string;
  label: string;
  value: string;
  error?: string;
  disabled?: boolean;
  onChange: (v: string) => void;
}) {
  const errorId = `${id}-error`;
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={id} className="text-xs font-medium text-[color:var(--text-secondary)]">
        {label}
      </label>
      <Input
        id={id}
        className="max-w-[160px]"
        inputMode="numeric"
        aria-invalid={error ? true : undefined}
        aria-describedby={describedBy(!!error && errorId)}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
      />
      <FieldError id={errorId} error={error} />
    </div>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(PerformanceScreen, "errors.resources.performanceSettings");
