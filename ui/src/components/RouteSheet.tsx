import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { useDiscardGuard } from "@/components/DiscardGuard";
import { FormSkeleton } from "@/components/LoadingState";
import { LoadError } from "@/components/LoadError";
import {
  newTarget,
  refusalOf,
  TargetEditor,
  weightValid,
  type DraftTarget,
  type RefusalField,
  type ServerRefusal,
} from "@/components/ModelSheet";
import { StrategyHint } from "@/components/StrategyHint";
import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import { FieldError } from "@/components/ui/field-error";
import { FieldLabel } from "@/components/ui/field-label";
import { FormSection } from "@/components/ui/form-section";
import {
  Sheet,
  SheetActions,
  SheetBody,
  SheetError,
  SheetFooter,
  SheetHeader,
} from "@/components/ui/sheet";
import { SwitchRow } from "@/components/ui/switch-row";
import {
  fetchRouteTargets,
  updateRoute,
  type ProviderRow,
  type RouteRow,
  type RouteTargetInput,
  type UpdateRouteInput,
} from "@/lib/api";
import { useErrorVisibility } from "@/lib/error-visibility";
import { providersUsableFrom } from "@/lib/provider-scope";
import { strategyOptions } from "@/lib/strategies";
import { errorDetail, useToast } from "@/lib/toast";
import { useFormTelemetry } from "@/lib/ux-react";

interface Draft {
  strategy: string;
  enabled: boolean;
  targets: DraftTarget[];
}

type FieldKey = RefusalField;

/**
 * The target list as the control plane takes it.
 *
 * A stored `upstream_model` goes back exactly as it was read, so the entry
 * matches the stored target by (provider, upstream model) and keeps its id.
 * That is why this does not reuse the model sheet's `upstreamFor`, which turns
 * a model equal to the route's name into "absent": for a stored target that
 * spells the name out, absent is a different key and the target would be
 * replaced with a new id (#2235).
 */
function targetsToApi(targets: DraftTarget[]): RouteTargetInput[] {
  return targets.map((tg) => ({
    provider_id: tg.providerId,
    upstream_model: tg.upstream.trim() === "" ? null : tg.upstream,
    weight: Number(tg.weight),
  }));
}

export interface RouteSheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** the route being edited; the sheet is closed while there is none */
  route: RouteRow | null;
  /** every provider of the org; the sheet offers the ones the route's project may use */
  providers: ProviderRow[];
  onDone: () => void;
}

/**
 * Edit a route's strategy, enabled flag and whole target list (#2134).
 *
 * The three travel in one `PUT /api/v1/routes/{id}`, applied in one transaction
 * by the control plane, so a refused weight leaves the route as it was rather
 * than half-edited. The name, parameters, pricing and the rest stay in the
 * model sheet.
 */
export function RouteSheet({
  open,
  onOpenChange,
  route,
  providers: orgProviders,
  onDone,
}: RouteSheetProps) {
  const { t } = useTranslation();
  const toast = useToast();
  const queryClient = useQueryClient();
  const providers = React.useMemo(
    () => providersUsableFrom(orgProviders, route?.project_id),
    [orgProviders, route?.project_id],
  );

  const targets = useQuery({
    queryKey: ["route-targets", route?.id],
    queryFn: () => fetchRouteTargets(route!.id),
    enabled: open && !!route,
  });

  const [draft, setDraft] = React.useState<Draft>({ strategy: "", enabled: true, targets: [] });
  const [routingOpen, setRoutingOpen] = React.useState(true);
  const [refusal, setRefusal] = React.useState<ServerRefusal | null>(null);
  const visibility = useErrorVisibility<FieldKey>();
  const initialRef = React.useRef("");
  const seededRef = React.useRef(false);
  React.useEffect(() => {
    if (!open || !route) {
      seededRef.current = false;
      return;
    }
    if (seededRef.current || !targets.isSuccess) return;
    seededRef.current = true;
    const d: Draft = {
      strategy: route.strategy,
      enabled: route.enabled,
      // every stored target, with its upstream model as stored
      targets: targets.data.map((tg) =>
        newTarget(tg.provider_id, tg.upstream_model ?? "", String(tg.weight), tg.id),
      ),
    };
    setDraft(d);
    setRefusal(null);
    visibility.reset();
    initialRef.current = JSON.stringify(d);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, route, targets.isSuccess, targets.data]);

  const dirty = initialRef.current !== "" && JSON.stringify(draft) !== initialRef.current;
  const ux = useFormTelemetry("route-edit", open, { dirty });

  // -- validation: the same three rules the model sheet states ---------------
  const providerRowInvalid = (tg: DraftTarget) => tg.providerId === "";
  const weightRowInvalid = (tg: DraftTarget) =>
    !weightValid(tg.weight) || (refusal?.field === "weight" && refusal.key === tg.key);
  const errTargets =
    draft.targets.length === 0 || refusal?.field === "targets"
      ? t("modelSheet.errors.targets")
      : "";
  const errProvider =
    draft.targets.some(providerRowInvalid) || refusal?.field === "provider"
      ? t("modelSheet.errors.provider")
      : "";
  const errWeight = draft.targets.some(weightRowInvalid) ? t("modelSheet.errors.weight") : "";
  const found: Record<FieldKey, string> = {
    targets: errTargets,
    provider: errProvider,
    weight: errWeight,
  };
  const problems = (Object.keys(found) as FieldKey[]).filter((key) => found[key]);
  // a refusal from the control plane is already news: show it without waiting
  // for the field to be visited
  const shownError = (key: FieldKey) =>
    visibility.shows(key) || refusal?.field === key ? found[key] : "";
  const summary = visibility.attempts > 0 && problems.length > 0;

  const fid = React.useId();
  const ids = {
    strategyLabel: `${fid}-strategy`,
    targetsLabel: `${fid}-targets-label`,
    targetsErr: `${fid}-targets-err`,
    providerErr: `${fid}-provider-err`,
    weightErr: `${fid}-weight-err`,
  };

  const save = useMutation({
    mutationFn: () => {
      const r = route!;
      const input: UpdateRouteInput = {};
      if (draft.enabled !== r.enabled) input.enabled = draft.enabled;
      if (draft.strategy !== r.strategy) input.strategy = draft.strategy;
      input.targets = targetsToApi(draft.targets);
      return updateRoute(r.id, input);
    },
    onSuccess: () => {
      ux.saved();
      queryClient.invalidateQueries({ queryKey: ["route-targets", route?.id] });
      queryClient.invalidateQueries({ queryKey: ["routes"] });
      queryClient.invalidateQueries({ queryKey: ["models"] });
      queryClient.invalidateQueries({ queryKey: ["config"] });
      toast.push({
        tone: "success",
        title: t("toast.saved"),
        detail: t("toast.savedDetail", { what: route?.model }),
      });
      onDone();
      onOpenChange(false);
    },
    onError: (error) => {
      ux.failed();
      const named = refusalOf(error, draft.targets);
      // a refusal about one line is said at that line; anything else (an
      // unknown provider, a scope mismatch, a 403) has no row to sit on
      if (named) {
        setRefusal(named);
        return;
      }
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: route?.model }),
        detail: errorDetail(error),
      });
    },
  });

  const { guard, close, locked, prompt } = useDiscardGuard({
    dirty,
    saving: save.isPending,
    onOpenChange,
  });

  const bodyRef = React.useRef<HTMLDivElement>(null);
  const requestSave = () => {
    save.reset();
    if (problems.length > 0) {
      visibility.attempt();
      return;
    }
    if (!dirty) {
      onOpenChange(false);
      return;
    }
    ux.submitted();
    save.mutate();
  };
  const attempts = visibility.attempts;
  React.useEffect(() => {
    if (attempts === 0) return;
    bodyRef.current
      ?.querySelector<HTMLElement>('[aria-invalid="true"], [data-error-anchor="true"]')
      ?.focus();
  }, [attempts]);
  // a server refusal moves focus to its line the same way
  React.useEffect(() => {
    if (!refusal) return;
    bodyRef.current
      ?.querySelector<HTMLElement>('[aria-invalid="true"], [data-error-anchor="true"]')
      ?.focus();
  }, [refusal]);

  const setTargets = (next: DraftTarget[]) => {
    setRefusal(null);
    setDraft((d) => ({ ...d, targets: next }));
    if (next.length < draft.targets.length) visibility.touch("targets");
  };
  const loading = targets.isPending && !!route;
  const rowNumber = (key?: string) => draft.targets.findIndex((tg) => tg.key === key) + 1;
  const weightMessage =
    refusal?.field === "weight" && !draft.targets.some((tg) => !weightValid(tg.weight))
      ? t("routeSheet.errors.weightRow", { n: rowNumber(refusal.key) })
      : shownError("weight");

  return (
    <Sheet open={open} onOpenChange={onOpenChange} onDismiss={guard}>
      <SheetHeader
        title={t("routeSheet.title")}
        subtitle={route?.model ?? ""}
        onClose={close}
        closeDisabled={locked}
      />
      <SheetBody ref={bodyRef}>
        {loading && <FormSkeleton fields={3} />}
        {targets.isError && (
          <LoadError
            error={targets.error}
            resource={t("errors.resources.routeTargets")}
            onRetry={() => void targets.refetch()}
            target="route-sheet-targets"
          />
        )}
        {targets.isSuccess && (
          <>
            <SwitchRow
              title={t("routeSheet.enabled")}
              hint={t("routeSheet.enabledHint")}
              checked={draft.enabled}
              onChange={(enabled) => setDraft((d) => ({ ...d, enabled }))}
            />
            <FormSection
              title={t("modelSheet.sections.routing")}
              info={t("modelSheet.sections.routingInfo")}
              open={routingOpen}
              onToggle={() => setRoutingOpen((o) => !o)}
              className="space-y-4"
            >
              <div className="space-y-1.5">
                <FieldLabel
                  label={t("modelSheet.routing.strategy")}
                  info={t("modelSheet.routing.strategyInfo")}
                  htmlFor={ids.strategyLabel}
                />
                <Combobox
                  id={ids.strategyLabel}
                  className="font-mono"
                  value={draft.strategy}
                  onChange={(strategy) => setDraft((d) => ({ ...d, strategy }))}
                  options={strategyOptions(draft.strategy).map((s) => ({ value: s, label: s }))}
                />
                <StrategyHint strategy={draft.strategy} />
              </div>
              <TargetEditor
                targets={draft.targets}
                providers={providers}
                hidden={orgProviders.length - providers.length}
                publicName={route?.model ?? ""}
                strategy={draft.strategy}
                labelId={ids.targetsLabel}
                emptyErrorId={ids.targetsErr}
                providerErrorId={ids.providerErr}
                weightErrorId={ids.weightErr}
                shown={{
                  empty: !!shownError("targets"),
                  provider: !!shownError("provider"),
                  weight: !!weightMessage,
                }}
                providerInvalid={providerRowInvalid}
                weightInvalid={weightRowInvalid}
                onChange={setTargets}
                onTouch={visibility.touch}
              />
              <FieldError id={ids.targetsErr} error={shownError("targets")} />
              <FieldError id={ids.providerErr} error={shownError("provider")} />
              <FieldError id={ids.weightErr} error={weightMessage} />
            </FormSection>
          </>
        )}
      </SheetBody>
      <SheetFooter>
        {summary && (
          <p
            role="alert"
            className="px-[22px] pt-2.5 text-xs leading-snug text-[color:var(--status-danger-text)]"
          >
            {t("routeSheet.errors.summary", { count: problems.length })}
          </p>
        )}
        <SheetError
          message={save.isError && !refusal ? (save.error as Error).message : undefined}
        />
        <SheetActions>
          <Button variant="ghost" disabled={locked} onClick={close}>
            {t("common.cancel")}
          </Button>
          <Button disabled={!targets.isSuccess || save.isPending} onClick={requestSave}>
            {t("routeSheet.save")}
          </Button>
        </SheetActions>
      </SheetFooter>
      {prompt}
    </Sheet>
  );
}
