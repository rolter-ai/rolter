import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { SettingsBody } from "@/components/screen";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Combobox } from "@/components/ui/combobox";
import { Badge } from "@/components/ui/badge";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { SettingsSaveBar } from "@/components/SettingsSaveBar";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { Switch } from "@/components/ui/switch";
import {
  fetchRuntimePolicy,
  updateRuntimePolicy,
  BACKPRESSURE_POLICIES,
  type BackpressurePolicy,
  type RuntimePolicyDto,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useToast } from "@/lib/toast";
import { translateErrors } from "@/lib/use-field-errors";
import { useSettingsForm } from "@/lib/use-settings-form";
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

// the catalog key naming each policy in the dropdown
const BACKPRESSURE_LABEL: Record<BackpressurePolicy, string> = {
  drop: "pages.performance.backpressureLabel.drop",
  block: "pages.performance.backpressureLabel.block",
  error: "pages.performance.backpressureLabel.error",
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

  const base = React.useId();
  const idOf = (key: FieldKey) => `${base}-${key}`;
  // an edit typed back to the stored value is no edit, and a draft is lost on
  // navigation (#2584), which the shared form state guards
  const source = React.useMemo(
    () => (policy.data ? fromDto(policy.data) : undefined),
    [policy.data],
  );
  const sf = useSettingsForm<FormState, FieldKey>({
    source,
    fieldErrors: (draft) => ({
      order: FIELD_ORDER,
      wire: WIRE_FIELDS,
      idOf,
      // a disabled field cannot be fixed, so no complaint about it holds Save
      skip: (key) => draft !== null && fieldDisabled(draft, key),
      local: draft ? translateErrors(validate(draft), t) : {},
    }),
  });
  const { draft: form, fields, changed } = sf;

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
      sf.commit(fromDto(dto));
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.performanceSettings") }),
      });
    },
    onError: (error) => {
      if (fields.refuse(error)) return;
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.performanceSettings") }),
        detail: errorText(error, t),
      });
    },
  });

  if (policy.isLoading) {
    return (
      <SettingsBody>
        <PanelSkeleton panels={3} height={132} />
      </SettingsBody>
    );
  }
  if (policy.isError) {
    return (
      <SettingsBody>
        <LoadError
          error={policy.error}
          resource={t("errors.resources.performanceSettings")}
          onRetry={() => void policy.refetch()}
          target="performance"
        />
      </SettingsBody>
    );
  }
  if (!form) return null;

  // the props every number field shares: its id, its value, its error, and an
  // edit that answers the server's complaint about that field
  const { set } = sf;
  const numberField = (key: FieldKey) => ({
    id: idOf(key),
    value: form[key],
    error: fields.errorOf(key),
    onChange: (v: string) => set({ [key]: v }),
  });
  const queue = form.queueEnabled;
  // a panel says it holds an edit, so the field that moved is findable among ten
  const editedBadge = (...keys: (keyof FormState)[]) =>
    keys.some((key) => changed.includes(key)) ? (
      <Badge tone="warning">{t("pages.performance.edited")}</Badge>
    ) : undefined;

  return (
    <SettingsBody onSubmit={() => fields.submit(() => save.mutate(form))}>
      <SettingsPanel
        title={t("pages.performance.retries.title")}
        description={t("pages.performance.retries.desc")}
        badge={editedBadge("retryMaxRetries", "retryBaseMs", "retryMaxMs")}
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
        badge={editedBadge("timeoutConnectS", "timeoutRequestS")}
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
        badge={editedBadge("queueCapacity", "queueWorkers", "queueBackpressure", "queueBlockMs")}
        unsaved={changed.includes("queueEnabled")}
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
        <Field
          label={t("pages.performance.queue.whenFull")}
          hint={t(BACKPRESSURE_COPY[form.queueBackpressure])}
          className="min-w-[200px]"
        >
          <Combobox
            id="perf-queue-backpressure"
            value={form.queueBackpressure}
            disabled={!queue}
            onChange={(picked) => set({ queueBackpressure: picked as BackpressurePolicy })}
            options={BACKPRESSURE_POLICIES.map((p) => ({
              value: p,
              label: t(BACKPRESSURE_LABEL[p]),
            }))}
          />
        </Field>
        <NumberField
          label={t("pages.performance.queue.blockTimeout")}
          {...numberField("queueBlockMs")}
          disabled={!queue || form.queueBackpressure !== "block"}
        />
      </SettingsPanel>

      <SettingsSaveBar form={sf} pending={save.isPending} updatedAt={policy.data?.updated_at} />
    </SettingsBody>
  );
}

// a screen row is a `Field`: it names the input, and an error under it is the
// input's description, so a screen reader hears the reason on the field (#2123)
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
  return (
    <Field label={label} error={error}>
      <Input
        id={id}
        className="max-w-[160px]"
        inputMode="numeric"
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
      />
    </Field>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(
  PerformanceScreen,
  "errors.resources.performanceSettings",
  "runtime_policy:read",
);
