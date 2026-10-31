import { ArrowDown, ArrowUp, Plus } from "lucide-react";
import * as React from "react";
import { Trans, useTranslation } from "react-i18next";
import { Link } from "react-router";

import { Button } from "@/components/ui/button";
import { Combobox } from "@/components/ui/combobox";
import { DeleteIconButton } from "@/components/ui/delete-icon-button";
import { describedBy, FieldError } from "@/components/ui/field-error";
import { FieldLabel } from "@/components/ui/field-label";
import { Input } from "@/components/ui/input";
import { ProviderLogo } from "@/components/ui/provider-logo";
import { SurfacePanel } from "@/components/ui/surface-panel";
import { TakesOverField } from "@/components/TakesOverField";
import type { ProviderRow } from "@/lib/api";
import { useFormat } from "@/lib/i18n/format";
import { shareBarColor } from "@/lib/share-bars";
import { usesWeights } from "@/lib/strategies";
import {
  MAX_VARIANT_TARGETS,
  MAX_VARIANTS,
  moved,
  newVariant,
  newVariantTarget,
  nextVariantName,
  trafficSplit,
  wholeWeight,
  type DraftVariant,
  type DraftVariantTarget,
  type VariantRefusal,
  type VariantValidation,
} from "@/lib/route-variants";

export interface RouteVariantEditorProps {
  variants: DraftVariant[];
  providers: ProviderRow[];
  /** the route's public name, the placeholder of a blank upstream model */
  publicName: string;
  /** the route's strategy, which also picks the leading target inside a variant */
  strategy: string;
  validation: VariantValidation;
  /**
   * every variant target is a fallback, so the route could never dial
   * anything: a route with variants dials only them (#3487)
   */
  onlyFallbacks?: boolean;
  /** show the validation, once a save was attempted */
  showErrors: boolean;
  /** a field the control plane refused, said at its line whatever `showErrors` is */
  refusal: VariantRefusal | null;
  onChange: (next: DraftVariant[]) => void;
  /** what the saved variants actually served, shown under the split preview (#3342) */
  observed?: React.ReactNode;
  /**
   * what stands in for the list while there are no variants; the caller owns
   * it, since it owns the read the empty state is paired with
   */
  empty: React.ReactNode;
}

/**
 * The weighted variants of a route (#391): named buckets, each with a share of
 * traffic and its own ordered targets.
 *
 * The order of the variants is the fallback chain after the sampled primary,
 * and the order of a variant's targets is the order they are tried, so both
 * lists move a line up or down rather than sorting. The preview above the list
 * is the split the gateway applies to first attempts, computed the way it
 * samples: a weight of 0 takes no first attempts while another variant has a
 * positive weight (#3341), and a set of only zeroes is shared equally.
 */
export function RouteVariantEditor({
  variants,
  providers,
  publicName,
  strategy,
  validation,
  onlyFallbacks = false,
  showErrors,
  refusal,
  onChange,
  observed,
  empty,
}: RouteVariantEditorProps) {
  const { t } = useTranslation();
  const fid = React.useId();
  const labelId = `${fid}-label`;
  const setErrId = `${fid}-set-err`;
  const firstProvider = providers[0]?.id ?? "";

  const update = (i: number, patch: Partial<DraftVariant>) =>
    onChange(variants.map((v, idx) => (idx === i ? { ...v, ...patch } : v)));
  const add = () => onChange([...variants, newVariant(nextVariantName(variants), firstProvider)]);

  // `variants` names more than one rule of the API, so its refusal is said in
  // the control plane's own words
  const setError =
    refusal?.field === "variants"
      ? (refusal.message ?? t("routeSheet.variants.errors.refused"))
      : showErrors && validation.tooMany
        ? t("routeSheet.variants.errors.tooMany", { max: MAX_VARIANTS })
        : showErrors && onlyFallbacks
          ? t("fallbackTargets.errors.variantsAllRestricted")
          : "";
  const full = variants.length >= MAX_VARIANTS;
  const weightsIgnored = !usesWeights(strategy) && variants.some((v) => v.targets.length > 1);

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between gap-2">
        <FieldLabel
          label={t("routeSheet.variants.title")}
          info={t("routeSheet.variants.info")}
          id={labelId}
        />
        <Button
          size="sm"
          variant="outline"
          disabled={providers.length === 0 || full}
          title={full ? t("routeSheet.variants.full", { max: MAX_VARIANTS }) : undefined}
          aria-describedby={describedBy(setError && setErrId)}
          // an error about the whole set, refused or found here, has no field
          // of its own to take focus (#3510)
          data-error-anchor={!!setError || undefined}
          onClick={add}
        >
          <Plus className="h-3.5 w-3.5" />
          {t("routeSheet.variants.add")}
        </Button>
      </div>
      {/* a variant's target needs a provider, which this sheet cannot make, so
          the way out is a link to where it can, as for the route's own targets */}
      {providers.length === 0 && (
        <p className="text-xs text-muted-foreground">
          <Trans
            i18nKey="modelSheet.targets.noProviders"
            components={[
              <Link
                key="providers"
                to="/providers"
                className="text-foreground underline underline-offset-2 hover:no-underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              />,
            ]}
          />
        </p>
      )}
      {variants.length === 0 ? (
        empty
      ) : (
        <>
          <p className="text-xs leading-snug text-muted-foreground">
            {t("routeSheet.variants.overridesTargets")}
          </p>
          <SplitPreview variants={variants} />
          {observed}
          {weightsIgnored && (
            <p className="text-xs leading-snug text-muted-foreground">
              <Trans
                i18nKey="routeSheet.variants.weightsIgnored"
                values={{ strategy }}
                components={[<span key="strategy" className="font-mono text-foreground" />]}
              />
            </p>
          )}
          <FieldError id={setErrId} error={setError} />
          <ol aria-labelledby={labelId} className="space-y-3">
            {variants.map((v, i) => (
              <VariantCard
                key={v.key}
                n={i + 1}
                count={variants.length}
                variant={v}
                color={shareBarColor(i)}
                providers={providers}
                publicName={publicName}
                validation={validation}
                showErrors={showErrors}
                refusal={refusal?.variantKey === v.key ? refusal : null}
                onChange={(patch) => update(i, patch)}
                onMove={(by) => onChange(moved(variants, i, by))}
                onRemove={() => onChange(variants.filter((_, idx) => idx !== i))}
              />
            ))}
          </ol>
        </>
      )}
    </div>
  );
}

/**
 * The share of first attempts each variant gets: one stacked bar, and the
 * figures under it. Nothing is drawn while a weight is not a whole number yet;
 * a 0 is drawn the way the gateway applies it (`trafficSplit`).
 */
function SplitPreview({ variants }: { variants: DraftVariant[] }) {
  const { t } = useTranslation();
  const fmt = useFormat();
  const ready = variants.every((v) => wholeWeight(v.weight));
  if (!ready) {
    return (
      <p className="text-xs text-muted-foreground">{t("routeSheet.variants.previewPending")}</p>
    );
  }
  const shares = trafficSplit(variants.map((v) => Number(v.weight)));
  const zeroes = variants.some((v) => Number(v.weight) === 0);
  return (
    <div className="space-y-2" data-testid="variant-split">
      <div
        aria-hidden="true"
        className="flex h-2 overflow-hidden rounded-full bg-[color:var(--surface-subtle)]"
      >
        {shares.map((share, i) => (
          <div
            key={variants[i].key}
            className="h-full"
            // a CSS length, never a localized percentage
            style={{ width: `${share * 100}%`, background: shareBarColor(i) }}
          />
        ))}
      </div>
      <ul
        aria-label={t("routeSheet.variants.previewLabel")}
        className="flex flex-wrap gap-x-4 gap-y-1 font-mono text-xs"
      >
        {shares.map((share, i) => (
          <li key={variants[i].key} className="flex items-center gap-1.5">
            <span
              aria-hidden="true"
              className="size-2 rounded-full"
              style={{ background: shareBarColor(i) }}
            />
            <span className="text-[color:var(--text-secondary)]">
              {variants[i].name.trim() || t("routeSheet.variants.unnamed", { n: i + 1 })}
            </span>
            <span className="text-foreground">{fmt.percent(share, 0)}</span>
          </li>
        ))}
      </ul>
      {zeroes && (
        <p className="text-xs leading-snug text-muted-foreground">
          {t("routeSheet.variants.zeroWeightNote")}
        </p>
      )}
    </div>
  );
}

function VariantCard({
  n,
  count,
  variant,
  color,
  providers,
  publicName,
  validation,
  showErrors,
  refusal,
  onChange,
  onMove,
  onRemove,
}: {
  n: number;
  count: number;
  variant: DraftVariant;
  color: string;
  providers: ProviderRow[];
  publicName: string;
  validation: VariantValidation;
  showErrors: boolean;
  refusal: VariantRefusal | null;
  onChange: (patch: Partial<DraftVariant>) => void;
  onMove: (by: -1 | 1) => void;
  onRemove: () => void;
}) {
  const { t } = useTranslation();
  const fid = React.useId();
  const ids = {
    name: `${fid}-name-err`,
    weight: `${fid}-weight-err`,
    targets: `${fid}-targets-err`,
    provider: `${fid}-provider-err`,
    targetWeight: `${fid}-target-weight-err`,
    targetsLabel: `${fid}-targets`,
  };
  const found = validation.byVariant.get(variant.key);
  const shown = (key: string | undefined, refused: boolean) =>
    (showErrors && key) || refused
      ? t(`routeSheet.variants.errors.${key ?? "refused"}`, { max: MAX_VARIANT_TARGETS })
      : "";
  const nameError = shown(found?.name, refusal?.field === "name");
  const weightError = shown(found?.weight, refusal?.field === "weight");
  const targetsError = shown(found?.targets, refusal?.field === "targets");
  const providerError =
    (showErrors && !!found?.provider.size) || refusal?.field === "provider"
      ? t("routeSheet.variants.errors.provider")
      : "";
  const targetWeightError =
    (showErrors && !!found?.targetWeight.size) || refusal?.field === "targetWeight"
      ? t("routeSheet.variants.errors.targetWeight")
      : "";

  const updateTarget = (j: number, patch: Partial<DraftVariantTarget>) =>
    onChange({
      targets: variant.targets.map((tg, idx) => (idx === j ? { ...tg, ...patch } : tg)),
    });
  // name, then weight and the controls; below `sm` the name takes a line of its own
  const head =
    "grid grid-cols-[72px_auto_auto_auto] items-center gap-2 sm:grid-cols-[minmax(0,1fr)_72px_auto_auto_auto]";
  const row =
    "grid grid-cols-[minmax(0,1fr)_64px_auto_auto_auto] items-center gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)_64px_auto_auto_auto]";

  return (
    <li>
      <SurfacePanel
        aria-label={t("routeSheet.variants.unnamed", { n })}
        className="space-y-3 border-l-2 p-3"
        style={{ borderLeftColor: color }}
      >
        <div className={head}>
          <Input
            aria-label={t("routeSheet.variants.nameAria", { n })}
            className="col-span-4 font-mono sm:col-span-1"
            placeholder={t("routeSheet.variants.namePlaceholder")}
            value={variant.name}
            aria-invalid={!!nameError || undefined}
            aria-describedby={describedBy(nameError && ids.name)}
            onChange={(e) => onChange({ name: e.target.value })}
          />
          <Input
            aria-label={t("routeSheet.variants.weightAria", { n })}
            type="number"
            min={0}
            step={1}
            className="font-mono"
            value={variant.weight}
            aria-invalid={!!weightError || undefined}
            aria-describedby={describedBy(weightError && ids.weight)}
            onChange={(e) => onChange({ weight: e.target.value })}
          />
          <Button
            variant="ghost"
            size="icon"
            disabled={n === 1}
            aria-label={t("routeSheet.variants.moveUp", { n })}
            title={t("routeSheet.variants.moveUpTitle")}
            onClick={() => onMove(-1)}
          >
            <ArrowUp className="size-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            disabled={n === count}
            aria-label={t("routeSheet.variants.moveDown", { n })}
            title={t("routeSheet.variants.moveDownTitle")}
            onClick={() => onMove(1)}
          >
            <ArrowDown className="size-4" />
          </Button>
          <DeleteIconButton
            label={t("routeSheet.variants.remove", { n })}
            title={t("common.remove")}
            onClick={onRemove}
          />
        </div>
        <FieldError id={ids.name} error={nameError} />
        <FieldError id={ids.weight} error={weightError} />
        {Object.keys(variant.params).length > 0 && (
          <p className="text-xs text-muted-foreground">
            {t("routeSheet.variants.params", { count: Object.keys(variant.params).length })}
          </p>
        )}

        <div className="flex items-center justify-between gap-2">
          <span
            id={ids.targetsLabel}
            className="text-2xs uppercase tracking-[0.06em] text-[color:var(--text-subtle)]"
          >
            {t("routeSheet.variants.targets")}
          </span>
          <Button
            size="sm"
            variant="ghost"
            disabled={providers.length === 0 || variant.targets.length >= MAX_VARIANT_TARGETS}
            title={
              variant.targets.length >= MAX_VARIANT_TARGETS
                ? t("routeSheet.variants.targetsFull", { max: MAX_VARIANT_TARGETS })
                : undefined
            }
            aria-label={t("routeSheet.variants.addTargetAria", { n })}
            aria-describedby={describedBy(targetsError && ids.targets)}
            data-error-anchor={(!!targetsError && variant.targets.length === 0) || undefined}
            onClick={() =>
              onChange({
                targets: [...variant.targets, newVariantTarget(providers[0]?.id ?? "")],
              })
            }
          >
            <Plus className="h-3.5 w-3.5" />
            {t("routeSheet.variants.addTarget")}
          </Button>
        </div>
        <ol aria-labelledby={ids.targetsLabel} className="space-y-2">
          {variant.targets.map((tg, j) => {
            const m = j + 1;
            const badProvider =
              (showErrors && !!found?.provider.has(tg.key)) ||
              (refusal?.field === "provider" && refusal.targetKey === tg.key);
            const badWeight =
              (showErrors && !!found?.targetWeight.has(tg.key)) ||
              (refusal?.field === "targetWeight" && refusal.targetKey === tg.key);
            return (
              <li key={tg.key} className={row}>
                <Combobox
                  aria-label={t("routeSheet.variants.targetProviderAria", { n, m })}
                  className="col-span-5 font-mono sm:col-span-1"
                  value={tg.providerId}
                  placeholder={t("modelSheet.targets.providerPlaceholder")}
                  aria-invalid={badProvider || undefined}
                  aria-describedby={describedBy(badProvider && ids.provider)}
                  onChange={(providerId) => updateTarget(j, { providerId })}
                  options={providers.map((p) => ({
                    value: p.id,
                    label: p.name,
                    icon: <ProviderLogo kind={p.kind} name={p.name} />,
                  }))}
                />
                <Input
                  aria-label={t("routeSheet.variants.targetUpstreamAria", { n, m })}
                  className="min-w-0 font-mono"
                  placeholder={publicName || t("modelSheet.targets.upstreamPlaceholder")}
                  value={tg.upstream}
                  onChange={(e) => updateTarget(j, { upstream: e.target.value })}
                />
                <Input
                  aria-label={t("routeSheet.variants.targetWeightAria", { n, m })}
                  type="number"
                  min={0}
                  step={1}
                  className="font-mono"
                  value={tg.weight}
                  aria-invalid={badWeight || undefined}
                  aria-describedby={describedBy(badWeight && ids.targetWeight)}
                  onChange={(e) => updateTarget(j, { weight: e.target.value })}
                />
                <Button
                  variant="ghost"
                  size="icon"
                  disabled={j === 0}
                  aria-label={t("routeSheet.variants.targetMoveUp", { n, m })}
                  title={t("routeSheet.variants.moveUpTitle")}
                  onClick={() => onChange({ targets: moved(variant.targets, j, -1) })}
                >
                  <ArrowUp className="size-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  disabled={j === variant.targets.length - 1}
                  aria-label={t("routeSheet.variants.targetMoveDown", { n, m })}
                  title={t("routeSheet.variants.moveDownTitle")}
                  onClick={() => onChange({ targets: moved(variant.targets, j, 1) })}
                >
                  <ArrowDown className="size-4" />
                </Button>
                <DeleteIconButton
                  label={t("routeSheet.variants.targetRemove", { n, m })}
                  title={t("common.remove")}
                  onClick={() =>
                    onChange({ targets: variant.targets.filter((_, idx) => idx !== j) })
                  }
                />
                <TakesOverField
                  groupLabel={t("fallbackTargets.variantGroupAria", { n, m })}
                  value={tg.takesOver}
                  error={
                    refusal?.field === "takesOver" && refusal.targetKey === tg.key
                      ? t("fallbackTargets.errors.refused")
                      : undefined
                  }
                  onChange={(takesOver) => updateTarget(j, { takesOver })}
                />
              </li>
            );
          })}
        </ol>
        <FieldError id={ids.targets} error={targetsError} />
        <FieldError id={ids.provider} error={providerError} />
        <FieldError id={ids.targetWeight} error={targetWeightError} />
      </SurfacePanel>
    </li>
  );
}
