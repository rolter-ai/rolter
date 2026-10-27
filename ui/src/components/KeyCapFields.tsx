import * as React from "react";
import { useTranslation } from "react-i18next";

import { FieldError, describedBy } from "@/components/ui/field-error";
import { FieldLabel } from "@/components/ui/field-label";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import { SwitchRow } from "@/components/ui/switch-row";
import { PERIOD_KINDS, type PeriodKind } from "@/lib/budget-period";
import { useCurrencyCode } from "@/lib/currency";
import { capProblems, type CapDraft } from "@/lib/key-caps";

// the optional spend and rate caps a key is minted with (#1171), shared by the
// operator sheet (Keys) and the self-service one (Account) like the rest of
// KeyMintFields. both default to off: a key with no cap is the ordinary key

export function KeyCapFields({
  value,
  onChange,
}: {
  value: CapDraft;
  onChange: (value: CapDraft) => void;
}) {
  const { t } = useTranslation();
  const currency = useCurrencyCode();
  const ids = React.useId();
  const problems = capProblems(value);
  const patch = (next: Partial<CapDraft>) => onChange({ ...value, ...next });

  const limitId = `${ids}-limit`;
  const limitErrorId = `${ids}-limit-error`;
  const windowId = `${ids}-window`;
  const windowHintId = `${ids}-window-hint`;
  const rpmId = `${ids}-rpm`;
  const rpmErrorId = `${ids}-rpm-error`;
  const tpmId = `${ids}-tpm`;
  const tpmErrorId = `${ids}-tpm-error`;
  const rateErrorId = `${ids}-rate-error`;

  const limitError = problems.limit
    ? t(`keyMint.caps.limitProblem.${problems.limit}`, { currency })
    : undefined;
  const rateError = (problem: "invalid" | "range" | null) =>
    problem ? t(`keyMint.caps.rateProblem.${problem}`) : undefined;
  const rpmError = rateError(problems.rpm);
  const tpmError = rateError(problems.tpm);

  return (
    <div className="space-y-2" role="group" aria-label={t("keyMint.caps.title")}>
      <SwitchRow
        title={t("keyMint.caps.spend")}
        hint={t("keyMint.caps.spendHint")}
        checked={value.budgetOn}
        onChange={(budgetOn) => patch({ budgetOn })}
      />
      {value.budgetOn && (
        <div className="space-y-3 px-1">
          <div className="space-y-1.5">
            <FieldLabel label={t("keyMint.caps.limit", { currency })} htmlFor={limitId} required />
            <Input
              id={limitId}
              inputMode="decimal"
              autoComplete="off"
              value={value.limit}
              onChange={(e) => patch({ limit: e.target.value })}
              placeholder={t("keyMint.caps.limitPlaceholder")}
              aria-invalid={!!limitError}
              aria-describedby={describedBy(!!limitError && limitErrorId)}
            />
            <FieldError id={limitErrorId} error={limitError} />
          </div>
          <div className="space-y-1.5">
            <FieldLabel label={t("keyMint.caps.window")} id={windowId} />
            <Segmented<PeriodKind>
              labelledBy={windowId}
              describedBy={windowHintId}
              value={value.period}
              onChange={(period) => patch({ period })}
              options={PERIOD_KINDS.map((kind) => ({
                value: kind,
                label: t(`pages.limits.periodOptions.${kind}`),
              }))}
            />
            <p id={windowHintId} className="text-xs text-muted-foreground">
              {t(`pages.limits.periodDescriptions.${value.period}`)}
            </p>
          </div>
        </div>
      )}
      <SwitchRow
        title={t("keyMint.caps.rate")}
        hint={t("keyMint.caps.rateHint")}
        checked={value.rateOn}
        onChange={(rateOn) => patch({ rateOn })}
      />
      {value.rateOn && (
        <div className="space-y-1.5 px-1">
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <FieldLabel label={t("keyMint.caps.rpm")} htmlFor={rpmId} />
              <Input
                id={rpmId}
                inputMode="numeric"
                autoComplete="off"
                value={value.rpm}
                onChange={(e) => patch({ rpm: e.target.value })}
                placeholder={t("keyMint.caps.noCap")}
                aria-invalid={!!rpmError}
                aria-describedby={describedBy(!!rpmError && rpmErrorId)}
              />
              <FieldError id={rpmErrorId} error={rpmError} />
            </div>
            <div className="space-y-1.5">
              <FieldLabel label={t("keyMint.caps.tpm")} htmlFor={tpmId} />
              <Input
                id={tpmId}
                inputMode="numeric"
                autoComplete="off"
                value={value.tpm}
                onChange={(e) => patch({ tpm: e.target.value })}
                placeholder={t("keyMint.caps.noCap")}
                aria-invalid={!!tpmError}
                aria-describedby={describedBy(!!tpmError && tpmErrorId)}
              />
              <FieldError id={tpmErrorId} error={tpmError} />
            </div>
          </div>
          <FieldError
            id={rateErrorId}
            error={problems.rateEmpty ? t("keyMint.caps.rateEmpty") : undefined}
          />
        </div>
      )}
    </div>
  );
}
