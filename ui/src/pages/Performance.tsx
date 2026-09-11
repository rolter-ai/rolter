import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import {
  fetchRuntimePolicy,
  updateRuntimePolicy,
  BACKPRESSURE_POLICIES,
  type BackpressurePolicy,
  type RuntimePolicyDto,
} from "@/lib/api";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

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
  if (!inRange(form.queueCapacity, 1, 100_000)) {
    return `${key}queueCapacity`;
  }
  if (!inRange(form.queueWorkers, 1, 2_048)) return `${key}queueWorkers`;
  if (!inRange(form.queueBlockMs, 0, 120_000)) {
    return `${key}blockTimeout`;
  }
  // blocking with a zero timeout would park callers forever
  if (form.queueBackpressure === "block" && Number(form.queueBlockMs) === 0) {
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
  useErrorState(!!policy.error, "performance");

  const [form, setForm] = React.useState<FormState | null>(null);
  React.useEffect(() => {
    if (policy.data && form === null) {
      setForm(fromDto(policy.data));
    }
  }, [policy.data, form]);

  const save = useMutation({
    mutationFn: (f: FormState) =>
      updateRuntimePolicy({
        retry_max_retries: Number(f.retryMaxRetries),
        retry_base_ms: Number(f.retryBaseMs),
        retry_max_ms: Number(f.retryMaxMs),
        timeout_connect_s: Number(f.timeoutConnectS),
        timeout_request_s: Number(f.timeoutRequestS),
        queue_enabled: f.queueEnabled,
        queue_capacity: Number(f.queueCapacity),
        queue_workers: Number(f.queueWorkers),
        queue_backpressure: f.queueBackpressure,
        queue_block_ms: Number(f.queueBlockMs),
      }),
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
      <Card
        title={t("pages.performance.retries.title")}
        desc={t("pages.performance.retries.desc")}
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
      </Card>

      <Card
        title={t("pages.performance.timeouts.title")}
        desc={t("pages.performance.timeouts.desc")}
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
      </Card>

      <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div className="flex items-start gap-4">
          <div className="min-w-0 flex-1">
            <span className="text-sm font-medium">
              {t("pages.performance.queue.title")}
            </span>
            <p className="mt-1 text-sm text-muted-foreground">
              {t("pages.performance.queue.desc")}
            </p>
          </div>
          <Switch
            checked={form.queueEnabled}
            aria-label={t("pages.performance.queue.toggleAria")}
            onCheckedChange={(v) => set({ queueEnabled: v })}
          />
        </div>
        {/* a disabled fieldset rather than a dimmed div: every control inside
            already carries `disabled`, and fading a live div drags its labels and
            hints below 4.5:1 while telling assistive tech nothing (#1181) */}
        <fieldset
          className="flex min-w-0 flex-wrap gap-4"
          disabled={!queue}
          style={{ opacity: queue ? 1 : 0.55 }}
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
            <label htmlFor="perf-queue-backpressure" className="text-xs font-medium text-[color:var(--text-secondary)]">
              {t("pages.performance.queue.whenFull")}
            </label>
            <Select
              id="perf-queue-backpressure"
              value={form.queueBackpressure}
              disabled={!queue}
              aria-label={t("pages.performance.queue.whenFull")}
              onChange={(e) =>
                set({ queueBackpressure: e.target.value as BackpressurePolicy })
              }
            >
              {BACKPRESSURE_POLICIES.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </Select>
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
        </fieldset>
      </section>

      <div className="sticky bottom-0 flex items-center justify-end gap-3 border-t border-[color:var(--border-subtle)] bg-background py-3">
        {localError && <span className="text-xs text-[color:var(--status-danger-text)]">{localError}</span>}
        <Button
          disabled={save.isPending || localError !== null}
          onClick={() => save.mutate(form)}
        >
          {save.isPending ? t("common.saving") : t("common.saveChanges")}
        </Button>
      </div>
    </div>
  );
}

function Card({
  title,
  desc,
  children,
}: {
  title: string;
  desc: string;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-3.5 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
      <div>
        <span className="text-sm font-medium">{title}</span>
        <p className="mt-1 text-sm text-muted-foreground">{desc}</p>
      </div>
      <div className="flex flex-wrap gap-4">{children}</div>
    </section>
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
