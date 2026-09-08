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
  fetchAdaptiveRoutingPolicy,
  updateAdaptiveRoutingPolicy,
  MAX_ADAPTIVE_MIN_SAMPLES,
  MAX_ADAPTIVE_WEIGHT,
  MAX_EXPLORATION_RATIO,
  type AdaptiveRoutingPolicyDto,
} from "@/lib/api";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

interface FormState {
  enabled: boolean;
  latencyWeight: string;
  costWeight: string;
  loadWeight: string;
  explorationRatio: string;
  minSamples: string;
}

const fromDto = (dto: AdaptiveRoutingPolicyDto): FormState => ({
  enabled: dto.enabled,
  latencyWeight: String(dto.latency_weight),
  costWeight: String(dto.cost_weight),
  loadWeight: String(dto.load_weight),
  explorationRatio: String(dto.exploration_ratio),
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
function validate(form: FormState): string | null {
  const weights = WEIGHTS.map(([key]) => Number(form[key]));
  if (weights.some((w) => !Number.isFinite(w) || w < 0 || w > MAX_ADAPTIVE_WEIGHT)) {
    return "pages.adaptiveSettings.validation.weightRange";
  }
  // an all-zero blend does not stop adaptive routing, it turns the strategy
  // into a random balancer — a much less obvious thing to read off a dashboard
  if (weights.every((w) => w <= 0)) {
    return "pages.adaptiveSettings.validation.weightPositive";
  }
  const ratio = Number(form.explorationRatio);
  if (!Number.isFinite(ratio) || ratio < 0 || ratio > MAX_EXPLORATION_RATIO) {
    return "pages.adaptiveSettings.validation.ratioRange";
  }
  const samples = Number(form.minSamples);
  if (
    !Number.isInteger(samples) ||
    samples < 0 ||
    samples > MAX_ADAPTIVE_MIN_SAMPLES
  ) {
    return "pages.adaptiveSettings.validation.samplesRange";
  }
  return null;
}

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
  useErrorState(!!policy.error, "adaptive-settings");

  const [form, setForm] = React.useState<FormState | null>(null);
  React.useEffect(() => {
    if (policy.data && form === null) {
      setForm(fromDto(policy.data));
    }
  }, [policy.data, form]);

  const save = useMutation({
    mutationFn: (f: FormState) =>
      updateAdaptiveRoutingPolicy({
        enabled: f.enabled,
        latency_weight: Number(f.latencyWeight),
        cost_weight: Number(f.costWeight),
        load_weight: Number(f.loadWeight),
        exploration_ratio: Number(f.explorationRatio),
        min_samples: Number(f.minSamples),
      }),
    onSuccess: (dto) => {
      queryClient.setQueryData(["adaptive-routing-policy"], dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: ["adaptive-routing-policy"] });
      setForm(fromDto(dto));
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.adaptiveSettings") }),
      });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.adaptiveSettings") }),
        detail: errorDetail(error),
      });
    },
  });

  if (policy.isLoading) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <PanelSkeleton panels={3} height={112} />
      </div>
    );
  }
  if (policy.isError) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <LoadError
          error={policy.error}
          resource={t("errors.resources.adaptiveSettings")}
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
  const localError = localErrorKey
    ? t(localErrorKey, {
        maxWeight: MAX_ADAPTIVE_WEIGHT,
        maxRatio: MAX_EXPLORATION_RATIO,
        maxSamples: MAX_ADAPTIVE_MIN_SAMPLES,
      })
    : null;
  const affected = policy.data?.affected_routes ?? [];
  const total = WEIGHTS.reduce((a, [key]) => a + (Number(form[key]) || 0), 0);

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      <section className="flex items-start gap-4 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div className="flex-1">
          <span className="text-sm font-medium">
            {t("pages.adaptiveSettings.title")}
          </span>
          <p className="mt-1 text-sm text-muted-foreground">
            <Trans
              i18nKey="pages.adaptiveSettings.killSwitch"
              components={[<code key="strategy" className="font-mono text-xs" />]}
            />
          </p>
          {/* the blast radius, so the switch is never flipped blind */}
          <div className="mt-2.5 flex flex-wrap items-center gap-1.5">
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
                  <Badge key={model} tone="outline" className="font-mono">
                    {model}
                  </Badge>
                ))}
              </>
            )}
          </div>
        </div>
        <Switch
          aria-label={t("pages.adaptiveSettings.toggleAria")}
          checked={form.enabled}
          onCheckedChange={(enabled) => set({ enabled })}
        />
      </section>

      <section className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">
            {t("pages.adaptiveSettings.weightsTitle")}
          </span>
          <p className="mt-1 text-sm text-muted-foreground">
            <Trans
              i18nKey="pages.adaptiveSettings.weightsDesc"
              components={[<span key="range" className="font-mono text-xs" />]}
            />
          </p>
        </div>
        {WEIGHTS.map(([key, name]) => {
          const label = t(`pages.adaptiveSettings.weights.${name}.label`);
          const hint = t(`pages.adaptiveSettings.weights.${name}.hint`);
          const value = Number(form[key]) || 0;
          const share = total > 0 ? Math.round((value / total) * 100) : 0;
          return (
            <div key={key} className="flex items-center gap-3">
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
                value={form[key]}
                onChange={(e) => set({ [key]: e.target.value } as Partial<FormState>)}
              />
            </div>
          );
        })}
      </section>

      <section className="flex flex-col gap-3 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
        <div>
          <span className="text-sm font-medium">
            {t("pages.adaptiveSettings.explorationTitle")}
          </span>
          <p className="mt-1 text-sm text-muted-foreground">
            {t("pages.adaptiveSettings.explorationDesc", {
              maxRatio: MAX_EXPLORATION_RATIO,
            })}
          </p>
        </div>
        <div className="flex items-center gap-3">
          <span className="flex-1 text-sm">
            {t("pages.adaptiveSettings.explorationRatio")}
          </span>
          <Input
            className="w-[92px]"
            inputMode="decimal"
            aria-label={t("pages.adaptiveSettings.explorationRatio")}
            value={form.explorationRatio}
            onChange={(e) => set({ explorationRatio: e.target.value })}
          />
        </div>
        <div className="flex items-center gap-3">
          <span className="flex-1 text-sm">
            {t("pages.adaptiveSettings.warmUpSamples")}
          </span>
          <Input
            className="w-[92px]"
            inputMode="numeric"
            aria-label={t("pages.adaptiveSettings.warmUpSamples")}
            value={form.minSamples}
            onChange={(e) => set({ minSamples: e.target.value })}
          />
        </div>
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

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(AdaptiveSettingsScreen, "errors.resources.adaptiveSettings");
