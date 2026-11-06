import { useTranslation } from "react-i18next";

import { Tooltip } from "@/components/ui/tooltip";

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

  // a muted dash with the reason as its note: the screen states the count once
  // above the table, so a warning badge on every row repeated it (#3659)
  if (priced === false) {
    return (
      <div className="flex justify-end">
        <Tooltip
          className="font-mono text-xs text-[color:var(--text-subtle)]"
          content={t("pages.models.unpriced.hint")}
        >
          <span aria-hidden>—</span>
          <span className="sr-only">{t("pages.models.unpriced.badge")}</span>
        </Tooltip>
      </div>
    );
  }

  return (
    <span className="text-right font-mono text-xs text-[color:var(--text-secondary)]">
      {inPrice} · {outPrice}
    </span>
  );
}
