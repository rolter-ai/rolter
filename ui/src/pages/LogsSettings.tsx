import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { SettingsBody } from "@/components/screen";
import { AlertTriangle } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { AnalyticsUnavailable } from "@/components/AnalyticsUnavailable";
import { DocsLink } from "@/components/DocsLink";
import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Field } from "@/components/ui/field";
import { FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import { SaveBar } from "@/components/ui/save-bar";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import {
  AnalyticsUnavailableError,
  fetchAnalyticsSummary,
  fetchLoggingSettings,
  updateLoggingSettings,
  type LoggingSettingsDto,
} from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { errorText } from "@/lib/error-copy";
import { parseSamplingPercent, sampleShare, samplingPercentText } from "@/lib/sampling";
import { useToast } from "@/lib/toast";
import { translateErrors, useFieldErrors } from "@/lib/use-field-errors";
import { useScreenReady } from "@/lib/ux-react";

interface FormState {
  samplePercent: string;
  captureEnabled: boolean;
  maxBytes: string;
  redactFields: string;
  models: string;
  virtualKeyIds: string;
  retentionDays: string;
  payloadRetentionHours: string;
  uiEvents: boolean;
}

const splitList = (value: string) =>
  value
    .split(",")
    .map((v) => v.trim())
    .filter(Boolean);

// the API takes a 0..1 fraction; operators think in percent, so the field is
// percent and the conversion happens at the edge
const fromDto = (dto: LoggingSettingsDto): FormState => ({
  samplePercent: samplingPercentText(dto.sample_rate),
  captureEnabled: dto.payload_capture_enabled,
  maxBytes: String(dto.payload_capture_max_bytes),
  redactFields: dto.payload_capture_redact_fields.join(", "),
  models: dto.payload_capture_models.join(", "),
  virtualKeyIds: dto.payload_capture_virtual_key_ids.join(", "),
  retentionDays: String(dto.retention_days),
  payloadRetentionHours: String(dto.payload_retention_hours),
  uiEvents: dto.ui_events,
});

// the typed percent as the 0..1 rate, or null while it holds nothing usable: a
// blank or malformed field never becomes a rate, so clearing it cannot save a
// policy that logs everything or nothing
const sampleRateOf = (text: string) => {
  const parsed = parseSamplingPercent(text);
  return parsed.ok ? parsed.rate : null;
};

const validMaxBytes = (value: string) => {
  const n = Number(value);
  return Number.isInteger(n) && n >= 0 && n <= 1_048_576;
};

// mirrors the server's validation so a bad value is caught before the round
// trip; the server stays the authority and its message is surfaced on reject.
// returns a catalog key, translated by the caller
// every failing field is reported at once, keyed by field (#2096)
type FieldKey = "samplePercent" | "maxBytes" | "retentionDays" | "payloadRetentionHours";
type FieldErrors = Partial<Record<FieldKey, string>>;

// the order the fields sit in, so focus lands on the first one that is wrong
const FIELD_ORDER: FieldKey[] = [
  "samplePercent",
  "maxBytes",
  "retentionDays",
  "payloadRetentionHours",
];

// the wire names a 400 opens with, mapped to the field they belong to
const WIRE_FIELDS: Record<string, FieldKey> = {
  sample_rate: "samplePercent",
  payload_capture_max_bytes: "maxBytes",
  retention_days: "retentionDays",
  payload_retention_hours: "payloadRetentionHours",
};

function validate(form: FormState): FieldErrors {
  const errors: FieldErrors = {};
  if (!parseSamplingPercent(form.samplePercent).ok) {
    errors.samplePercent = "pages.logsSettings.errors.sampleRange";
  }
  // the field is disabled while capture is off, so a bad value there could
  // not be fixed; it is re-checked once capture is switched back on
  if (form.captureEnabled && !validMaxBytes(form.maxBytes)) {
    errors.maxBytes = "pages.logsSettings.errors.maxBytes";
  }
  const days = Number(form.retentionDays);
  const daysOk = Number.isInteger(days) && days >= 1 && days <= 3650;
  if (!daysOk) errors.retentionDays = "pages.logsSettings.errors.retentionDays";
  const hours = Number(form.payloadRetentionHours);
  if (!Number.isInteger(hours) || hours < 1 || hours > 8760) {
    errors.payloadRetentionHours = "pages.logsSettings.errors.payloadRetentionHours";
  } else if (daysOk && hours > days * 24) {
    // raw bodies are the sensitive half: keeping them past the metadata they
    // belong to would leak prompt content the operator meant to expire
    errors.payloadRetentionHours = "pages.logsSettings.errors.payloadOutlivesLog";
  }
  return errors;
}

// global request-log policy, persisted via /api/v1/logging-settings (superadmin
// only). controls how much traffic is sampled, whether raw payloads are
// captured, what is redacted from them, how long each is kept (#537), and
// whether the dashboard's own UX events are accepted at all (#1748)
function LogsSettingsScreen() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const format = useFormat();
  const settings = useQuery({
    queryKey: ["logging-settings"],
    queryFn: fetchLoggingSettings,
    retry: false,
  });

  // whether anything receives the log. there is no flag for it: the analytics
  // reads answer 503 when the control plane has no clickhouse_url, which is the
  // signal every other screen reads (#2092). a one-hour summary is the cheapest
  // read; any other failure says nothing about the deployment, so no notice
  const analytics = useQuery({
    queryKey: ["logging-settings", "analytics-probe"],
    queryFn: () => fetchAnalyticsSummary({ since: new Date(Date.now() - 3_600_000).toISOString() }),
    retry: false,
    staleTime: 60_000,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `settings` is the query the user is actually waiting on for this screen
  useScreenReady(!settings.isLoading);

  const [form, setForm] = React.useState<FormState | null>(null);
  const sampleHintId = React.useId();
  const sampleWarningId = React.useId();
  const base = React.useId();
  const ids: Record<FieldKey, string> = {
    samplePercent: `${base}-sample`,
    maxBytes: "logs-max-bytes",
    retentionDays: "logs-retention-days",
    payloadRetentionHours: "logs-payload-retention-hours",
  };
  React.useEffect(() => {
    if (settings.data && form === null) {
      setForm(fromDto(settings.data));
    }
  }, [settings.data, form]);

  const fields = useFieldErrors<FieldKey>({
    order: FIELD_ORDER,
    wire: WIRE_FIELDS,
    idOf: (key) => ids[key],
    local: form ? translateErrors(validate(form), t) : {},
  });

  const save = useMutation({
    mutationFn: (f: FormState) =>
      updateLoggingSettings({
        // validated before a save can start, so the fallback is never sent
        sample_rate: sampleRateOf(f.samplePercent) ?? settings.data?.sample_rate ?? 1,
        payload_capture_enabled: f.captureEnabled,
        // an unusable value is only reachable with capture off; keep what is stored
        payload_capture_max_bytes: validMaxBytes(f.maxBytes)
          ? Number(f.maxBytes)
          : (settings.data?.payload_capture_max_bytes ?? 0),
        payload_capture_redact_fields: splitList(f.redactFields),
        payload_capture_models: splitList(f.models),
        payload_capture_virtual_key_ids: splitList(f.virtualKeyIds),
        retention_days: Number(f.retentionDays),
        payload_retention_hours: Number(f.payloadRetentionHours),
        ui_events: f.uiEvents,
      }),
    onSuccess: (dto) => {
      queryClient.setQueryData(["logging-settings"], dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: ["logging-settings"] });
      setForm(fromDto(dto));
      fields.clear();
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.logsSettings") }),
      });
    },
    onError: (error) => {
      if (fields.refuse(error)) return;
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.logsSettings") }),
        detail: errorText(error, t),
      });
    },
  });

  if (settings.isLoading) {
    return (
      <SettingsBody>
        <PanelSkeleton panels={4} height={104} />
      </SettingsBody>
    );
  }
  if (settings.isError) {
    return (
      <SettingsBody>
        <LoadError
          error={settings.error}
          resource={t("errors.resources.logsSettings")}
          onRetry={() => void settings.refetch()}
          target="logs-settings"
        />
      </SettingsBody>
    );
  }
  if (!form) return null;

  const set = (patch: Partial<FormState>) => {
    setForm((f) => (f ? { ...f, ...patch } : f));
  };
  // an edit answers the server's complaint about that field
  const edit = (key: FieldKey, value: string) => {
    set({ [key]: value });
    fields.edit(key);
  };
  const capture = form.captureEnabled;
  const dirty =
    JSON.stringify(form) !== JSON.stringify(fromDto(settings.data as LoggingSettingsDto));
  const changedAt = settings.data ? new Date(settings.data.updated_at) : null;

  return (
    <SettingsBody>
      {analytics.error instanceof AnalyticsUnavailableError && (
        <div className="flex flex-col gap-2">
          <AnalyticsUnavailable error={analytics.error} i18nKey="pages.logsSettings.noAnalytics" />
          <DocsLink page="logSettings" label={t("docs.link.logSettings")} />
        </div>
      )}
      <SettingsPanel
        title={t("pages.logsSettings.sampleRate")}
        description={<span id={sampleHintId}>{t("pages.logsSettings.sampleRateHint")}</span>}
      >
        <div className="flex w-full flex-wrap items-center gap-2">
          <Input
            className="max-w-[120px]"
            inputMode="decimal"
            aria-label={t("pages.logsSettings.sampleRatePercent")}
            {...fields.inputProps("samplePercent", sampleHintId, sampleWarningId)}
            value={form.samplePercent}
            onChange={(e) => edit("samplePercent", e.target.value)}
          />
          <span className="text-sm text-muted-foreground">
            {t("pages.logsSettings.percentOfRequests")}
          </span>
        </div>
        <FieldError id={fields.errorId("samplePercent")} error={fields.errorOf("samplePercent")} />
        <SampledLogWarning id={sampleWarningId} rate={sampleRateOf(form.samplePercent)} />
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.logsSettings.capture")}
        description={t("pages.logsSettings.captureHint")}
        dimmed={!capture}
        action={
          <Switch
            checked={form.captureEnabled}
            aria-label={t("pages.logsSettings.captureAria")}
            onCheckedChange={(v) => set({ captureEnabled: v })}
          />
        }
      >
        {/* what this switch actually does, said where it is thrown rather than
            three cards further down (#954). the numbers come from the live form
            state, so the summary reflects the edit in progress — including one
            that has not been saved yet */}
        <p
          role="note"
          className="w-full rounded-[8px] border border-dashed border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] p-3 text-xs leading-relaxed text-muted-foreground"
        >
          {capture
            ? t("pages.logsSettings.captureOnSummary", {
                bytes: form.maxBytes || "0",
                hours: form.payloadRetentionHours || "0",
                redacted:
                  splitList(form.redactFields).join(", ") || t("pages.logsSettings.nothing"),
                models: splitList(form.models).join(", ") || t("pages.logsSettings.everyModel"),
              })
            : t("pages.logsSettings.captureOffSummary")}
        </p>
        <div className="flex min-w-0 flex-col gap-1.5">
          <label
            htmlFor="logs-max-bytes"
            className="text-xs font-medium text-[color:var(--text-secondary)]"
          >
            {t("pages.logsSettings.maxBytes")}
          </label>
          <Input
            className="max-w-[180px]"
            inputMode="numeric"
            disabled={!capture}
            aria-label={t("pages.logsSettings.maxBytes")}
            {...fields.inputProps("maxBytes")}
            value={form.maxBytes}
            onChange={(e) => edit("maxBytes", e.target.value)}
          />
          <FieldError id={fields.errorId("maxBytes")} error={fields.errorOf("maxBytes")} />
          <span className="text-xs text-[color:var(--text-subtle)]">
            {t("pages.logsSettings.maxBytesHint")}
          </span>
        </div>
        {/* these only narrow what the switch captures, so they sit inside it
            and go dead with it (#2094) */}
        <FilterField
          label={t("pages.logsSettings.redacted")}
          hint={t("pages.logsSettings.redactedHint")}
          value={form.redactFields}
          placeholder={t("pages.logsSettings.redactedPlaceholder")}
          onChange={(v) => set({ redactFields: v })}
        />
        <FilterField
          label={t("pages.logsSettings.onlyModels")}
          hint={t("pages.logsSettings.onlyModelsHint")}
          value={form.models}
          placeholder={t("pages.logsSettings.onlyModelsPlaceholder")}
          onChange={(v) => set({ models: v })}
        />
        <FilterField
          label={t("pages.logsSettings.onlyKeys")}
          hint={t("pages.logsSettings.onlyKeysHint")}
          value={form.virtualKeyIds}
          placeholder="0b7f1e2a-1c3d-4e5f-8a9b-0c1d2e3f4a5b"
          onChange={(v) => set({ virtualKeyIds: v })}
        />
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.logsSettings.retention")}
        description={t("pages.logsSettings.retentionHint")}
      >
        <div className="flex flex-wrap gap-4">
          <div className="flex flex-col gap-1.5">
            <label
              htmlFor="logs-retention-days"
              className="text-xs font-medium text-[color:var(--text-secondary)]"
            >
              {t("pages.logsSettings.retentionDays")}
            </label>
            <Input
              className="max-w-[140px]"
              inputMode="numeric"
              aria-label={t("pages.logsSettings.retentionDaysAria")}
              {...fields.inputProps("retentionDays")}
              value={form.retentionDays}
              onChange={(e) => edit("retentionDays", e.target.value)}
            />
            <FieldError
              id={fields.errorId("retentionDays")}
              error={fields.errorOf("retentionDays")}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label
              htmlFor="logs-payload-retention-hours"
              className="text-xs font-medium text-[color:var(--text-secondary)]"
            >
              {t("pages.logsSettings.payloadRetentionHours")}
            </label>
            <Input
              className="max-w-[140px]"
              inputMode="numeric"
              aria-label={t("pages.logsSettings.payloadRetentionHoursAria")}
              {...fields.inputProps("payloadRetentionHours")}
              value={form.payloadRetentionHours}
              onChange={(e) => edit("payloadRetentionHours", e.target.value)}
            />
            <FieldError
              id={fields.errorId("payloadRetentionHours")}
              error={fields.errorOf("payloadRetentionHours")}
            />
          </div>
        </div>
      </SettingsPanel>

      {/* the deployment-level opt-out for the UX stream. before #1748 it was a
          toml-only key that a postgres-backed control plane never read. a
          panel with its switch in the header, like capture above (#2094) */}
      <SettingsPanel
        title={t("pages.logsSettings.uiEvents")}
        description={t("pages.logsSettings.uiEventsHint")}
        action={
          <Switch
            checked={form.uiEvents}
            aria-label={t("pages.logsSettings.uiEvents")}
            onCheckedChange={(v) => set({ uiEvents: v })}
          />
        }
      />

      <SaveBar
        invalid={fields.invalid.length}
        pending={save.isPending}
        disabled={!dirty}
        status={
          changedAt && !Number.isNaN(changedAt.getTime())
            ? t("pages.logsSettings.updatedAt", { when: format.dateTime(changedAt) })
            : undefined
        }
        onSave={() => fields.submit(() => save.mutate(form))}
      />
    </SettingsBody>
  );
}

/**
 * What a sample rate below 100 % does to every figure read from the request log
 * (#2088).
 *
 * The gateway drops an unsampled row before it reaches ClickHouse and stamps
 * each kept row with the number of requests it stands for. The analytics reads
 * scale counts, spend and tokens by it (#2239), so at 25 % the Dashboard's spend
 * is an estimate built from a quarter of the traffic, not a quarter of the
 * spend. The warning follows the form state, so
 * it shows for an edit that has not been saved yet as well as for a saved rate
 * on load.
 *
 * The status region stays mounted and only its content comes and goes: a live
 * region inserted together with its text is not reliably announced, and the
 * warning appearing mid-edit is what a screen-reader user needs to hear.
 */
function SampledLogWarning({ id, rate }: { id: string; rate: number | null }) {
  const { t } = useTranslation();
  const format = useFormat();
  const below = rate !== null && rate < 1;
  const share = rate !== null && below ? sampleShare(rate) : null;
  return (
    <div id={id} role="status">
      {below && (
        <p className="flex items-start gap-1.5 text-xs text-[color:var(--status-warning-text)]">
          <AlertTriangle aria-hidden className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>
            <span className="font-medium">
              {share
                ? t("pages.logsSettings.sampled.title", {
                    count: share.denominator,
                    numerator: format.number(share.numerator),
                    denominator: format.number(share.denominator),
                  })
                : t("pages.logsSettings.sampled.noneTitle")}
            </span>{" "}
            {share
              ? t("pages.logsSettings.sampled.body")
              : t("pages.logsSettings.sampled.noneBody")}
          </span>
        </p>
      )}
    </div>
  );
}

function FilterField({
  label,
  hint,
  value,
  placeholder,
  onChange,
}: {
  label: string;
  hint: string;
  value: string;
  placeholder: string;
  onChange: (v: string) => void;
}) {
  return (
    <Field label={label} hint={hint} className="w-full">
      <Textarea
        className="min-h-[64px] w-full font-mono text-xs"
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
      />
    </Field>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(LogsSettingsScreen, "errors.resources.logsSettings");
