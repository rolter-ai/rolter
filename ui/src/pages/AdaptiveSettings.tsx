import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { SettingsBody } from "@/components/screen";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";
import { Link } from "react-router";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { FieldError } from "@/components/ui/field-error";
import { Input } from "@/components/ui/input";
import { SettingsSaveBar } from "@/components/SettingsSaveBar";
import { SettingsPanel } from "@/components/ui/settings-panel";
import { Switch } from "@/components/ui/switch";
import {
  fetchAdaptiveRoutingPolicy,
  updateAdaptiveRoutingPolicy,
  MAX_ADAPTIVE_MIN_SAMPLES,
  MAX_ADAPTIVE_WEIGHT,
  MAX_EXPLORATION_RATIO,
  type AdaptiveRoutingPolicyDto,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { useToast } from "@/lib/toast";
import { translateErrors } from "@/lib/use-field-errors";
import { useSettingsForm } from "@/lib/use-settings-form";
import { useScreenReady } from "@/lib/ux-react";

interface FormState {
  enabled: boolean;
  latencyWeight: string;
  costWeight: string;
  loadWeight: string;
  explorationRatio: string;
  minSamples: string;
}

// the ratio travels as a fraction (0.05) and is typed as the percent it means
// (5); rounding off the binary noise keeps 0.07 from reading as 7.000000000000001
const toPercent = (ratio: number): string => String(Number((ratio * 100).toFixed(4)));

const fromDto = (dto: AdaptiveRoutingPolicyDto): FormState => ({
  enabled: dto.enabled,
  latencyWeight: String(dto.latency_weight),
  costWeight: String(dto.cost_weight),
  loadWeight: String(dto.load_weight),
  explorationRatio: toPercent(dto.exploration_ratio),
  minSamples: String(dto.min_samples),
});

// each weight names its catalog keys; the copy itself lives in en.json
const WEIGHTS = [
  ["latencyWeight", "latency"],
  ["costWeight", "cost"],
  ["loadWeight", "load"],
] as const;

// mirrors the server's validation so a bad blend is caught before the round
// trip; the server stays the authority and its message is surfaced on reject.
// it names a catalog key rather than carrying english copy — the screen renders
// it, which is where `t` lives
// every failing field is reported at once, keyed by field (#2651)
type FieldKey = Exclude<keyof FormState, "enabled">;
type FieldErrors = Partial<Record<FieldKey, string>>;

// the order the fields sit in, so focus lands on the first one that is wrong
const FIELD_ORDER: FieldKey[] = [
  "latencyWeight",
  "costWeight",
  "loadWeight",
  "explorationRatio",
  "minSamples",
];

// the wire names a 400 opens with, mapped to the field they belong to
const WIRE_FIELDS: Record<string, FieldKey> = {
  latency_weight: "latencyWeight",
  cost_weight: "costWeight",
  load_weight: "loadWeight",
  exploration_ratio: "explorationRatio",
  min_samples: "minSamples",
};

function validate(form: FormState): FieldErrors {
  const errors: FieldErrors = {};
  let weightsOk = true;
  for (const [key] of WEIGHTS) {
    const w = Number(form[key]);
    if (!Number.isFinite(w) || w < 0 || w > MAX_ADAPTIVE_WEIGHT) {
      errors[key] = "pages.adaptiveSettings.validation.weightRange";
      weightsOk = false;
    }
  }
  // an all-zero blend does not stop adaptive routing, it turns the strategy
  // into a random balancer — a much less obvious thing to read off a dashboard.
  // the blend has no single culprit, so the message sits on the first weight
  if (weightsOk && WEIGHTS.every(([key]) => Number(form[key]) <= 0)) {
    errors.latencyWeight = "pages.adaptiveSettings.validation.weightPositive";
  }
  const percent = Number(form.explorationRatio);
  if (
    form.explorationRatio.trim() === "" ||
    !Number.isFinite(percent) ||
    percent < 0 ||
    percent > MAX_EXPLORATION_PERCENT
  ) {
    errors.explorationRatio = "pages.adaptiveSettings.validation.ratioRange";
  }
  const samples = Number(form.minSamples);
  if (!Number.isInteger(samples) || samples < 0 || samples > MAX_ADAPTIVE_MIN_SAMPLES) {
    errors.minSamples = "pages.adaptiveSettings.validation.samplesRange";
  }
  return errors;
}

const MAX_EXPLORATION_PERCENT = Number((MAX_EXPLORATION_RATIO * 100).toFixed(4));

// the bounds the validation copy interpolates
const LIMITS = {
  maxWeight: MAX_ADAPTIVE_WEIGHT,
  maxPercent: MAX_EXPLORATION_PERCENT,
  maxSamples: MAX_ADAPTIVE_MIN_SAMPLES,
};

// global adaptive-routing policy, persisted via /api/v1/adaptive-routing-policy
// (superadmin only). only the ratio between the weights matters — the blend is
// a weighted sum of signals each scored in [0, 1] (#544, #565)
function AdaptiveSettingsScreen() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const policy = useQuery({
    queryKey: ["adaptive-routing-policy"],
    queryFn: fetchAdaptiveRoutingPolicy,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `policy` is the query the user is actually waiting on for this screen
  useScreenReady(!policy.isLoading);

  const base = React.useId();
  const ids: Record<FieldKey, string> = {
    latencyWeight: `${base}-latency`,
    costWeight: `${base}-cost`,
    loadWeight: `${base}-load`,
    explorationRatio: `${base}-ratio`,
    minSamples: `${base}-samples`,
  };
  const source = React.useMemo(
    () => (policy.data ? fromDto(policy.data) : undefined),
    [policy.data],
  );
  const sf = useSettingsForm<FormState, FieldKey>({
    source,
    fieldErrors: (draft) => ({
      order: FIELD_ORDER,
      wire: WIRE_FIELDS,
      idOf: (key) => ids[key],
      local: draft ? translateErrors(validate(draft), t, LIMITS) : {},
    }),
  });
  const { draft: form, fields } = sf;

  const save = useMutation({
    mutationFn: (f: FormState) =>
      updateAdaptiveRoutingPolicy({
        enabled: f.enabled,
        latency_weight: Number(f.latencyWeight),
        cost_weight: Number(f.costWeight),
        load_weight: Number(f.loadWeight),
        exploration_ratio: Number(f.explorationRatio) / 100,
        min_samples: Number(f.minSamples),
      }),
    onSuccess: (dto) => {
      queryClient.setQueryData(["adaptive-routing-policy"], dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: ["adaptive-routing-policy"] });
      sf.commit(fromDto(dto));
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.adaptiveSettings") }),
      });
    },
    onError: (error) => {
      if (fields.refuse(error)) return;
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.adaptiveSettings") }),
        detail: errorText(error, t),
      });
    },
  });

  if (policy.isLoading) {
    return (
      <SettingsBody>
        <PanelSkeleton panels={3} height={112} />
      </SettingsBody>
    );
  }
  if (policy.isError) {
    return (
      <SettingsBody>
        <LoadError
          error={policy.error}
          resource={t("errors.resources.adaptiveSettings")}
          onRetry={() => void policy.refetch()}
          target="adaptive-settings"
        />
      </SettingsBody>
    );
  }
  if (!form) return null;

  // an edit answers the server's complaint about that field
  const { set, changed } = sf;
  const edit = (key: FieldKey, value: string) => set({ [key]: value });
  const affected = policy.data?.affected_routes ?? [];
  const total = WEIGHTS.reduce((a, [key]) => a + (Number(form[key]) || 0), 0);

  return (
    <SettingsBody onSubmit={() => fields.submit(() => save.mutate(form))}>
      <SettingsPanel
        title={t("pages.adaptiveSettings.title")}
        unsaved={changed.includes("enabled")}
        description={
          <Trans
            i18nKey="pages.adaptiveSettings.killSwitch"
            components={[<code key="strategy" className="font-mono text-xs" />]}
          />
        }
        action={
          <Switch
            aria-label={t("pages.adaptiveSettings.toggleAria")}
            checked={form.enabled}
            onCheckedChange={(enabled) => set({ enabled })}
          />
        }
      >
        {/* the blast radius, so the switch is never flipped blind */}
        <div className="flex flex-wrap items-center gap-1.5">
          {affected.length === 0 ? (
            <span className="text-xs text-muted-foreground">
              {t("pages.adaptiveSettings.noAffectedRoutes")}
            </span>
          ) : (
            <>
              <span className="text-xs text-muted-foreground">
                {t("pages.adaptiveSettings.governsRoutes", {
                  count: affected.length,
                })}
              </span>
              {affected.map((model) => (
                <Badge key={model} tone="outline" className="font-mono text-xs">
                  {model}
                </Badge>
              ))}
            </>
          )}
        </div>
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.adaptiveSettings.weightsTitle")}
        description={
          <Trans
            i18nKey="pages.adaptiveSettings.weightsDesc"
            components={[<span key="range" className="font-mono text-xs" />]}
          />
        }
      >
        <div className="flex w-full flex-col gap-3">
          {WEIGHTS.map(([key, name]) => {
            const label = t(`pages.adaptiveSettings.weights.${name}.label`);
            const hint = t(`pages.adaptiveSettings.weights.${name}.hint`);
            const value = Number(form[key]) || 0;
            const share = total > 0 ? Math.round((value / total) * 100) : 0;
            return (
              <div key={key} className="flex flex-col gap-1.5">
                <div className="flex items-center gap-3">
                  <div className="flex-1">
                    <span className="text-sm">{label}</span>
                    <p className="text-xs text-muted-foreground">{hint}</p>
                  </div>
                  <span className="w-12 text-right font-mono text-xs text-muted-foreground">
                    {share}%
                  </span>
                  <Input
                    className="w-[92px]"
                    inputMode="decimal"
                    aria-label={t("pages.adaptiveSettings.weightAria", { label })}
                    {...fields.inputProps(key)}
                    value={form[key]}
                    onChange={(e) => edit(key, e.target.value)}
                  />
                </div>
                <FieldError id={fields.errorId(key)} error={fields.errorOf(key)} />
              </div>
            );
          })}
        </div>
      </SettingsPanel>

      <SettingsPanel
        title={t("pages.adaptiveSettings.explorationTitle")}
        description={t("pages.adaptiveSettings.explorationDesc", {
          maxPercent: MAX_EXPLORATION_PERCENT,
        })}
      >
        <div className="flex w-full flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <div className="flex items-center gap-3">
              <span className="flex-1 text-sm">{t("pages.adaptiveSettings.explorationRatio")}</span>
              <Input
                className="w-[92px]"
                inputMode="decimal"
                aria-label={t("pages.adaptiveSettings.explorationRatio")}
                {...fields.inputProps("explorationRatio")}
                value={form.explorationRatio}
                onChange={(e) => edit("explorationRatio", e.target.value)}
              />
              <span aria-hidden="true" className="w-4 font-mono text-xs text-muted-foreground">
                %
              </span>
            </div>
            <FieldError
              id={fields.errorId("explorationRatio")}
              error={fields.errorOf("explorationRatio")}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <div className="flex items-center gap-3">
              <span className="flex-1 text-sm">{t("pages.adaptiveSettings.warmUpSamples")}</span>
              <Input
                className="w-[92px]"
                inputMode="numeric"
                aria-label={t("pages.adaptiveSettings.warmUpSamples")}
                {...fields.inputProps("minSamples")}
                value={form.minSamples}
                onChange={(e) => edit("minSamples", e.target.value)}
              />
            </div>
            <FieldError id={fields.errorId("minSamples")} error={fields.errorOf("minSamples")} />
          </div>
        </div>
      </SettingsPanel>

      <SettingsSaveBar
        form={sf}
        pending={save.isPending}
        updatedAt={policy.data?.updated_at}
        leading={
          // a save has no visible effect here, so the way to the live view sits
          // beside it; the dashboard links back
          <Link
            to="/adaptive-dashboard"
            className="mr-auto rounded-sm text-sm font-medium text-foreground underline decoration-[color:var(--border-strong)] underline-offset-4 transition-colors hover:decoration-current focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            {t("pages.adaptiveSettings.viewDashboard")}
          </Link>
        }
      />
    </SettingsBody>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(
  AdaptiveSettingsScreen,
  "errors.resources.adaptiveSettings",
  "adaptive_routing_policy:read",
);
