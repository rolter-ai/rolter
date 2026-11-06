import { useTranslation } from "react-i18next";

import { Pill } from "@/components/screen";

/**
 * The per-Mtok price cell in the model catalogue (#969).
 *
 * A model with no price row is not a free model — its traffic is served and
 * recorded as `unpriced`, and its spend is simply missing from totals and
 * budgets. Rendering that as a dash would make it indistinguishable from a
 * column that merely has nothing to show, which is the confusion this cell
 * exists to remove.
 */
export function ModelPriceCell({
  priced,
  inPrice,
  outPrice,
}: {
  /**
   * whether a price row applied. `null` means the price catalogue could not be
   * read at all, which is not evidence that this model is unpriced — the cell
   * falls back to the plain figures rather than accusing every model.
   */
  priced: boolean | null;
  /** input price, already formatted; `—` when absent */
  inPrice: string;
  /** output price, already formatted; `—` when absent */
  outPrice: string;
}) {
  const { t } = useTranslation();

  if (priced === false) {
    return (
      <div className="flex justify-end">
        <Pill
          hint={t("pages.models.unpriced.hint")}
          color="var(--status-warning-text)"
          tint="var(--red-tint)"
          border="color-mix(in srgb, var(--status-warning) 32%, transparent)"
        >
          {t("pages.models.unpriced.badge")}
        </Pill>
      </div>
    );
  }

  return (
    <span className="text-right font-mono text-xs text-[color:var(--text-secondary)]">
      {inPrice} · {outPrice}
    </span>
  );
}
