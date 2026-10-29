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
import { RouteVariantEditor } from "@/components/RouteVariantEditor";
import { RouteVariantTraffic } from "@/components/RouteVariantTraffic";
import { StrategyPicker } from "@/components/StrategyPicker";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
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
  ApiError,
  fetchRouteTargets,
  fetchRouteVariants,
  setRouteVariants,
  updateRoute,
  type ProviderRow,
  type RouteRow,
  type RouteTargetInput,
  type UpdateRouteInput,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { allRestricted, onlyRestrictedDialled } from "@/lib/failure-classes";
import { useErrorVisibility } from "@/lib/error-visibility";
import { providersUsableFrom } from "@/lib/provider-scope";
import {
  validateVariants,
  variantRefusalOf,
  variantsFromRows,
  variantsToApi,
  type DraftVariant,
  type VariantRefusal,
} from "@/lib/route-variants";
import { useToast } from "@/lib/toast";
import { useFormTelemetry } from "@/lib/ux-react";

interface Draft {
  strategy: string;
  enabled: boolean;
  targets: DraftTarget[];
  /** the weighted variants (#391), saved by their own request */
  variants: DraftVariant[];
}

/** the part of the draft `PUT /api/v1/routes/{id}` saves */
const routePart = (d: Draft) =>
  JSON.stringify({ strategy: d.strategy, enabled: d.enabled, targets: d.targets });
/** the part `PUT /api/v1/routes/{id}/variants` saves */
const variantPart = (d: Draft) => JSON.stringify(d.variants);

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
    // always sent: an entry without it clears the stored set (#3319)
    takes_over: tg.takesOver,
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
 * Edit a route's strategy, enabled flag, whole target list (#2134) and weighted
 * variants (#391).
 *
 * The first three travel in one `PUT /api/v1/routes/{id}`, applied in one
 * transaction by the control plane, so a refused weight leaves the route as it
 * was rather than half-edited. The variants are their own resource and go in a
 * second `PUT /api/v1/routes/{id}/variants`, sent only when they changed. The
 * control plane judges each write against what is stored at that moment (a
 * route with variants dials only them, #3487), so the order is the one that
 * lets every end state it accepts land in one save (#3510): variants that stay
 * go first, since they decide whether the pool counts at all. A removal of
 * every variant goes first too while the stored ones hold only fallbacks, since
 * the pool is judged against them until they are gone, and last otherwise,
 * once the pool it hands the route to is stored. The name, parameters, pricing
 * and the rest stay in the model sheet.
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

  const variants = useQuery({
    queryKey: ["route-variants", route?.id],
    queryFn: () => fetchRouteVariants(route!.id),
    enabled: open && !!route,
  });

  const [draft, setDraft] = React.useState<Draft>({
    strategy: "",
    enabled: true,
    targets: [],
    variants: [],
  });
  const [routingOpen, setRoutingOpen] = React.useState(true);
  const [variantsOpen, setVariantsOpen] = React.useState(true);
  const [refusal, setRefusal] = React.useState<ServerRefusal | null>(null);
  const [variantRefusal, setVariantRefusal] = React.useState<VariantRefusal | null>(null);
  const visibility = useErrorVisibility<FieldKey>();
  const initialRef = React.useRef("");
  const initialRouteRef = React.useRef("");
  const initialVariantsRef = React.useRef("");
  const seededRef = React.useRef(false);
  React.useEffect(() => {
    if (!open || !route) {
      seededRef.current = false;
      return;
    }
    if (seededRef.current || !targets.isSuccess || !variants.isSuccess) return;
    seededRef.current = true;
    const d: Draft = {
      strategy: route.strategy,
      enabled: route.enabled,
      // every stored target, with its upstream model as stored
      targets: targets.data.map((tg) =>
        newTarget(
          tg.provider_id,
          tg.upstream_model ?? "",
          String(tg.weight),
          tg.id,
          tg.takes_over ?? [],
        ),
      ),
      variants: variantsFromRows(variants.data),
    };
    setDraft(d);
    setRefusal(null);
    setVariantRefusal(null);
    visibility.reset();
    initialRef.current = JSON.stringify(d);
    initialRouteRef.current = routePart(d);
    initialVariantsRef.current = variantPart(d);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, route, targets.isSuccess, targets.data, variants.isSuccess, variants.data]);

  const dirty = initialRef.current !== "" && JSON.stringify(draft) !== initialRef.current;
  const routeDirty = initialRouteRef.current !== "" && routePart(draft) !== initialRouteRef.current;
  const variantsDirty =
    initialVariantsRef.current !== "" && variantPart(draft) !== initialVariantsRef.current;
  const ux = useFormTelemetry("route-edit", open, { dirty });

  // -- validation: the same three rules the model sheet states ---------------
  const providerRowInvalid = (tg: DraftTarget) => tg.providerId === "";
  const weightRowInvalid = (tg: DraftTarget) =>
    !weightValid(tg.weight) || (refusal?.field === "weight" && refusal.key === tg.key);
  // a route with variants routes through them, so a list that was stored empty
  // may stay empty. one that held targets may not be emptied: the update cannot
  // send an empty list, and leaving it out would keep the stored targets
  const emptyAllowed = draft.variants.length > 0 && (targets.data?.length ?? 0) === 0;
  // a route left with nothing but fallbacks to dial could never dial
  // anything: the control plane refuses it as `all_targets_restricted` (#1584).
  // a route with variants dials only them, so its own list does not count
  // then, and the problem is said under the variants (#3487)
  const stranded = onlyRestrictedDialled(
    draft.targets.map((tg) => tg.takesOver),
    draft.variants.map((v) => v.targets.map((tg) => tg.takesOver)),
  );
  const variantsOnlyFallbacks = stranded && draft.variants.length > 0;
  // the variants as stored now hold only fallbacks, so the pool is judged
  // against a route that dials nothing until they change
  const storedSets = (variants.data ?? []).map((v) =>
    (v.targets ?? []).map((tg) => tg.takes_over ?? []),
  );
  const storedVariantsStranded = storedSets.length > 0 && allRestricted(storedSets.flat());
  const onlyFallbacks = stranded && draft.variants.length === 0;
  // a refusal of the whole list that is about the variants is said under them
  // (`onRefusal` below), so it never lands here while the route has variants
  const errTargets =
    onlyFallbacks || !!refusal?.allRestricted
      ? t("fallbackTargets.errors.allRestricted")
      : (draft.targets.length === 0 && !emptyAllowed) || refusal?.field === "targets"
        ? t("modelSheet.errors.targets")
        : "";
  const takesOverError = (tg: DraftTarget) =>
    refusal?.field === "takesOver" && refusal.key === tg.key
      ? t("fallbackTargets.errors.refused")
      : undefined;
  const errProvider =
    draft.targets.some(providerRowInvalid) || refusal?.field === "provider"
      ? t("modelSheet.errors.provider")
      : "";
  const errWeight = draft.targets.some(weightRowInvalid) ? t("modelSheet.errors.weight") : "";
  const found: Record<FieldKey, string> = {
    targets: errTargets,
    provider: errProvider,
    weight: errWeight,
    // said under the refused target's own set, not in the summary's count
    takesOver: "",
  };
  const problems = (Object.keys(found) as FieldKey[]).filter((key) => found[key]);
  const variantCheck = validateVariants(draft.variants);
  const problemCount = problems.length + variantCheck.count + Number(variantsOnlyFallbacks);
  // a refusal from the control plane is already news: show it without waiting
  // for the field to be visited
  const shownError = (key: FieldKey) =>
    visibility.shows(key) || refusal?.field === key ? found[key] : "";
  const summary = visibility.attempts > 0 && problemCount > 0;

  const fid = React.useId();
  const ids = {
    strategyLabel: `${fid}-strategy`,
    targetsLabel: `${fid}-targets-label`,
    targetsErr: `${fid}-targets-err`,
    providerErr: `${fid}-provider-err`,
    weightErr: `${fid}-weight-err`,
  };

  // which request a failure came from: a variants refusal names `variants…`
  // fields, which mean nothing to the target list
  const stageRef = React.useRef<"route" | "variants">("route");
  const save = useMutation({
    mutationFn: async () => {
      const r = route!;
      const saveVariants = async () => {
        stageRef.current = "variants";
        await setRouteVariants(r.id, variantsToApi(draft.variants));
        initialVariantsRef.current = variantPart(draft);
      };
      // variants that stay decide whether the pool is dialled, so they are
      // stored first: a pool PUT judged against stored variants that are only
      // fallbacks would be refused, though the variants being saved repair them
      // (#3510). a removal of every variant goes first for the same reason
      // while the stored ones strand the route; otherwise it waits for the
      // pool, since the variants PUT then judges the pool it hands the route to
      const variantsFirst = variantsDirty && (draft.variants.length > 0 || storedVariantsStranded);
      if (variantsFirst) await saveVariants();
      if (routeDirty) {
        stageRef.current = "route";
        const input: UpdateRouteInput = {};
        if (draft.enabled !== r.enabled) input.enabled = draft.enabled;
        if (draft.strategy !== r.strategy) input.strategy = draft.strategy;
        // an empty list is refused, and a route on variants needs none
        if (draft.targets.length > 0) input.targets = targetsToApi(draft.targets);
        await updateRoute(r.id, input);
        initialRouteRef.current = routePart(draft);
      }
      if (variantsDirty && !variantsFirst) await saveVariants();
    },
    onSuccess: () => {
      ux.saved();
      queryClient.invalidateQueries({ queryKey: ["route-targets", route?.id] });
      queryClient.invalidateQueries({ queryKey: ["route-variants", route?.id] });
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
      // the other request may have saved; the lists show what is stored now
      queryClient.invalidateQueries({ queryKey: ["routes"] });
      if (stageRef.current === "variants") {
        const named =
          error instanceof ApiError && error.code === "invalid_field" && error.field
            ? variantRefusalOf(error.field, draft.variants, error.message)
            : null;
        if (named) {
          setVariantRefusal(named);
          setVariantsOpen(true);
          return;
        }
        toast.push({
          tone: "error",
          title: t("toast.saveFailed", { what: route?.model }),
          detail: errorText(error, t),
        });
        return;
      }
      const named = refusalOf(error, draft.targets);
      // a route with variants dials only them, so a list refused for holding
      // only fallbacks is the variants' problem and is said beside them
      if (named?.allRestricted && draft.variants.length > 0) {
        setVariantRefusal({
          field: "variants",
          message: t("fallbackTargets.errors.variantsAllRestricted"),
        });
        setVariantsOpen(true);
        return;
      }
      // a refusal about one line is said at that line; anything else (an
      // unknown provider, a scope mismatch, a 403) has no row to sit on
      if (named) {
        setRefusal(named);
        return;
      }
      toast.push({
        tone: "error",
        title: t("toast.saveFailed", { what: route?.model }),
        detail: errorText(error, t),
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
    if (problemCount > 0) {
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
  // a server refusal moves focus to its line the same way, a variant's
  // included; the section is opened first, so the field is there to take it
  React.useEffect(() => {
    if (!refusal && !variantRefusal) return;
    bodyRef.current
      ?.querySelector<HTMLElement>('[aria-invalid="true"], [data-error-anchor="true"]')
      ?.focus();
  }, [refusal, variantRefusal]);

  const setTargets = (next: DraftTarget[]) => {
    setRefusal(null);
    setDraft((d) => ({ ...d, targets: next }));
    if (next.length < draft.targets.length) visibility.touch("targets");
  };
  const loading = (targets.isPending || variants.isPending) && !!route;
  const loaded = targets.isSuccess && variants.isSuccess;
  const setVariants = (next: DraftVariant[]) => {
    setVariantRefusal(null);
    setDraft((d) => ({ ...d, variants: next }));
  };
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
          // load-error-allow: the editor's seed read; an empty target list is a validation error here, not an empty state
          <LoadError
            error={targets.error}
            resource={t("errors.resources.routeTargets")}
            onRetry={() => void targets.refetch()}
            target="route-sheet-targets"
          />
        )}
        {variants.isError && (
          <LoadError
            error={variants.error}
            resource={t("errors.resources.routeVariants")}
            onRetry={() => void variants.refetch()}
            target="route-variants"
          />
        )}
        {loaded && (
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
                <StrategyPicker
                  id={ids.strategyLabel}
                  value={draft.strategy}
                  onChange={(strategy) => setDraft((d) => ({ ...d, strategy }))}
                />
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
                takesOverError={takesOverError}
                onChange={setTargets}
                onTouch={visibility.touch}
              />
              <FieldError id={ids.targetsErr} error={shownError("targets")} />
              <FieldError id={ids.providerErr} error={shownError("provider")} />
              <FieldError id={ids.weightErr} error={weightMessage} />
            </FormSection>
            <FormSection
              title={t("routeSheet.variants.title")}
              open={variantsOpen}
              onToggle={() => setVariantsOpen((o) => !o)}
            >
              <RouteVariantEditor
                variants={draft.variants}
                providers={providers}
                publicName={route?.model ?? ""}
                strategy={draft.strategy}
                validation={variantCheck}
                onlyFallbacks={variantsOnlyFallbacks}
                showErrors={visibility.attempts > 0}
                refusal={variantRefusal}
                onChange={setVariants}
                observed={
                  route && variants.data ? (
                    <RouteVariantTraffic
                      model={route.model}
                      projectId={route.project_id}
                      saved={variants.data}
                    />
                  ) : undefined
                }
                empty={
                  <EmptyState
                    inline
                    uxTarget="route-variants"
                    description={t("routeSheet.variants.empty")}
                  />
                }
              />
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
            {t("routeSheet.errors.summary", { count: problemCount })}
          </p>
        )}
        <SheetError
          message={
            save.isError && !refusal && !variantRefusal ? errorText(save.error, t) : undefined
          }
        />
        <SheetActions>
          <Button variant="ghost" disabled={locked} onClick={close}>
            {t("common.cancel")}
          </Button>
          <Button disabled={!loaded || save.isPending} onClick={requestSave}>
            {t("routeSheet.save")}
          </Button>
        </SheetActions>
      </SheetFooter>
      {prompt}
    </Sheet>
  );
}
