import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { superadminOnly } from "@/components/ForbiddenScreen";
import { LoadError } from "@/components/LoadError";
import { PanelSkeleton } from "@/components/LoadingState";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import {
  fetchFeatureFlags,
  updateFeatureFlags,
  FEATURE_FLAG_KEYS,
  type FeatureFlagKey,
  type FeatureFlagValues,
  type FeatureFlagsDto,
  type UnavailableFlagDto,
} from "@/lib/api";
import { errorDetail, useToast } from "@/lib/toast";
import { useErrorState, useScreenReady } from "@/lib/ux-react";

interface FlagCopy {
  title: string;
  desc: string;
}

// one entry per allowlisted flag; the order here is the order on screen
const COPY: Record<FeatureFlagKey, FlagCopy> = {
  response_cache: {
    title: "Response Cache",
    desc: "Serve repeated completions from the shared Redis cache instead of forwarding them upstream.",
  },
  cache_aware_routing: {
    title: "Cache-Aware Routing",
    desc: "Score targets by KV-cache affinity so a conversation keeps landing on the replica that already holds its prefix.",
  },
  circuit_breaker: {
    title: "Circuit Breaker",
    desc: "Trip a target out of the pool after repeated upstream failures and probe it before restoring traffic.",
  },
  active_health_checks: {
    title: "Active Health Checks",
    desc: "Poll targets in the background so an unhealthy one is parked before a live request discovers it.",
  },
  complexity_routing: {
    title: "Complexity Routing",
    desc: "Estimate prompt complexity and route cheap requests to smaller models.",
  },
  guardrails: {
    title: "Guardrails",
    desc: "Run the configured pre- and post-call guardrail rules on every request.",
  },
};

const toValues = (dto: FeatureFlagsDto): FeatureFlagValues =>
  Object.fromEntries(
    FEATURE_FLAG_KEYS.map((key) => [key, dto[key]]),
  ) as FeatureFlagValues;

// global feature flags, persisted via /api/v1/feature-flags (superadmin only).
// the server also reports which flags have no working subsystem in this
// deployment and rejects enabling them, so those render as unavailable rather
// than as a switch that silently does nothing (#535)
function FeatureFlagsScreen() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const toast = useToast();
  const flags = useQuery({
    queryKey: ["feature-flags"],
    queryFn: fetchFeatureFlags,
    retry: false,
  });

  // UX stream (#805). the screen key comes from the enclosing UxScreenProvider;
  // `flags` is the query the user is actually waiting on for this screen
  useScreenReady(!flags.isLoading);
  useErrorState(!!flags.error, "feature-flags");

  const [form, setForm] = React.useState<FeatureFlagValues | null>(null);
  React.useEffect(() => {
    if (flags.data && form === null) {
      setForm(toValues(flags.data));
    }
  }, [flags.data, form]);

  const save = useMutation({
    mutationFn: (values: FeatureFlagValues) => updateFeatureFlags(values),
    onSuccess: (dto) => {
      queryClient.setQueryData(["feature-flags"], dto);
      // the cached write alone left every other reader of this key on the
      // value it already had; the refetch is what makes the save stick (#1197)
      void queryClient.invalidateQueries({ queryKey: ["feature-flags"] });
      setForm(toValues(dto));
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: t("errors.resources.featureFlags") }),
      });
    },
    onError: (error) => {
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: t("errors.resources.featureFlags") }),
        detail: errorDetail(error),
      });
    },
  });

  if (flags.isLoading) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <PanelSkeleton panels={FEATURE_FLAG_KEYS.length} height={86} />
      </div>
    );
  }
  if (flags.isError) {
    return (
      <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
        <LoadError
          error={flags.error}
          resource={t("errors.resources.featureFlags")}
          onRetry={() => void flags.refetch()}
        />
      </div>
    );
  }
  if (!form) return null;

  const unavailable = flags.data?.unavailable ?? [];
  const reasonFor = (key: FeatureFlagKey) =>
    unavailable.find((u: UnavailableFlagDto) => u.flag === key)?.reason;

  const set = (key: FeatureFlagKey, value: boolean) => {
    setForm((f) => (f ? { ...f, [key]: value } : f));
  };

  return (
    <div className="mx-auto flex max-w-[840px] flex-col gap-3.5 p-[22px]">
      {FEATURE_FLAG_KEYS.map((key) => (
        <FlagCard
          key={key}
          title={COPY[key].title}
          desc={COPY[key].desc}
          checked={form[key]}
          unavailableReason={reasonFor(key)}
          onChange={(v) => set(key, v)}
        />
      ))}

      <div className="sticky bottom-0 flex items-center justify-end gap-3 border-t border-[color:var(--border-subtle)] bg-background py-3">
        <Button disabled={save.isPending} onClick={() => save.mutate(form)}>
          {save.isPending ? t("common.saving") : t("common.saveChanges")}
        </Button>
      </div>
    </div>
  );
}

// an unavailable flag stays visible but cannot be switched on: the server would
// reject it, and a live switch would imply the subsystem is running
function FlagCard({
  title,
  desc,
  checked,
  unavailableReason,
  onChange,
}: {
  title: string;
  desc: string;
  checked: boolean;
  unavailableReason?: string;
  onChange: (v: boolean) => void;
}) {
  const unavailable = unavailableReason !== undefined;
  return (
    <section className="flex items-start gap-4 rounded-[10px] border border-[color:var(--border-subtle)] p-4">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium">{title}</span>
          {unavailable && <Badge tone="warning">UNAVAILABLE</Badge>}
        </div>
        <p className="mt-1 text-sm text-muted-foreground">{desc}</p>
        {unavailable && (
          <p className="mt-1.5 text-[0.6875rem] text-[color:var(--text-subtle)]">
            {unavailableReason}
          </p>
        )}
      </div>
      <Switch
        checked={checked}
        disabled={unavailable}
        aria-label={title}
        onCheckedChange={onChange}
      />
    </section>
  );
}

// deployment-scoped settings: superadmin-only in the capability table, so a
// lesser caller sees the refusal instead of a screen that loads and then 403s
// (#1183)
export default superadminOnly(FeatureFlagsScreen, "errors.resources.featureFlags");
