import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
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
function validate(form: FormState): string | null {
  const key = "pages.performance.validation.";
  if (!inRange(form.retryMaxRetries, 0, 10)) return `${key}maxRetries`;
  if (!inRange(form.retryBaseMs, 0, 60_000)) return `${key}retryBase`;
  if (!inRange(form.retryMaxMs, 0, 600_000)) return `${key}retryCap`;
  if (Number(form.retryMaxMs) < Number(form.retryBaseMs)) {
    return `${key}retryCapBelowBase`;
  }
  if (!inRange(form.timeoutConnectS, 0, 300)) return `${key}connectTimeout`;
  if (!inRange(form.timeoutRequestS, 0, 3_600)) {
    return `${key}requestTimeout`;
  }
  // the queue fields are disabled while the queue is off, and the block
  // timeout while the policy is not `block`, so a bad value there could not be
  // fixed; each is re-checked once its control is enabled again (#2645)
  if (!form.queueEnabled) return null;
  if (!validCapacity(form.queueCapacity)) {
    return `${key}queueCapacity`;
  }
  if (!validWorkers(form.queueWorkers)) return `${key}queueWorkers`;
  if (form.queueBackpressure !== "block") return null;
  if (!validBlockMs(form.queueBlockMs)) {
    return `${key}blockTimeout`;
  }
  // blocking with a zero timeout would park callers forever
  if (Number(form.queueBlockMs) === 0) {
    return `${key}blockNeedsTimeout`;
  }
  return null;
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
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.performanceSettings") }),
      });
    },
    onError: (error) => {
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
  const localErrorKey = validate(form);
  const localError = localErrorKey ? t(localErrorKey) : null;
  const queue = form.queueEnabled;

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      <SettingsPanel
        title={t("pages.performance.retries.title")}
        description={t("pages.performance.retries.desc")}
      >
        <NumberField
          label={t("pages.performance.retries.maxRetries")}
          value={form.retryMaxRetries}
          onChange={(v) => set({ retryMaxRetries: v })}
        />
        <NumberField
          label={t("pages.performance.retries.baseBackoff")}
          value={form.retryBaseMs}
          onChange={(v) => set({ retryBaseMs: v })}
        />
        <NumberField
          label={t("pages.performance.retries.backoffCap")}
          value={form.retryMaxMs}
          onChange={(v) => set({ retryMaxMs: v })}
        />
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.performance.timeouts.title")}
        description={t("pages.performance.timeouts.desc")}
      >
        <NumberField
          label={t("pages.performance.timeouts.connect")}
          value={form.timeoutConnectS}
          onChange={(v) => set({ timeoutConnectS: v })}
        />
        <NumberField
          label={t("pages.performance.timeouts.request")}
          value={form.timeoutRequestS}
          onChange={(v) => set({ timeoutRequestS: v })}
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
          value={form.queueCapacity}
          disabled={!queue}
          onChange={(v) => set({ queueCapacity: v })}
        />
        <NumberField
          label={t("pages.performance.queue.workers")}
          value={form.queueWorkers}
          disabled={!queue}
          onChange={(v) => set({ queueWorkers: v })}
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
          value={form.queueBlockMs}
          disabled={!queue || form.queueBackpressure !== "block"}
          onChange={(v) => set({ queueBlockMs: v })}
        />
      </SettingsPanel>

      <div className="sticky bottom-0 flex items-center justify-end gap-3 border-t border-[color:var(--border-subtle)] bg-background py-3">
        {localError && (
          <span className="text-xs text-[color:var(--status-danger-text)]">{localError}</span>
        )}
        <Button disabled={save.isPending || localError !== null} onClick={() => save.mutate(form)}>
          {save.isPending ? t("common.saving") : t("common.saveChanges")}
        </Button>
      </div>
    </div>
  );
}

function NumberField({
  label,
  value,
  disabled = false,
  onChange,
}: {
  label: string;
  value: string;
  disabled?: boolean;
  onChange: (v: string) => void;
}) {
  const id = React.useId();
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={id} className="text-xs font-medium text-[color:var(--text-secondary)]">
        {label}
      </label>
      <Input
        id={id}
        className="max-w-[160px]"
        inputMode="numeric"
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(PerformanceScreen, "errors.resources.performanceSettings");
