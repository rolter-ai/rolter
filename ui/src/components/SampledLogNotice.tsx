import { Info } from "lucide-react";
import { useTranslation } from "react-i18next";

import { useFormat } from "@/lib/i18n/format";
import { sampleShare } from "@/lib/sampling";
import { useLogSampling } from "@/lib/use-log-sampling";

/**
 * Says that a screen reads a sampled request log (#2231).
 *
 * Below 100 % the gateway keeps one row in so many and stamps it with the
 * requests it stands for. The analytics reads scale counts, spend and tokens
 * by that weight (#2239), so on the Dashboard and Cost Attribution a figure is
 * an estimate, not a tally. The LLM Logs list is the sampled rows themselves:
 * a request missing from it was served all the same. Nothing renders at 100 %,
 * or when the rate cannot be read.
 */
export function SampledLogNotice({ kind }: { kind: "figures" | "rows" }) {
  const rate = useLogSampling();
  if (rate === null) return null;
  return <SampledLogNoticeView rate={rate} kind={kind} />;
}

/** The notice for a known rate below 1, apart from where the rate comes from. */
export function SampledLogNoticeView({ rate, kind }: { rate: number; kind: "figures" | "rows" }) {
  const { t } = useTranslation();
  const format = useFormat();
  const share = sampleShare(rate);
  return (
    <div
      role="status"
      data-testid="sampled-log-notice"
      className="flex items-start gap-2.5 rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-4 py-3"
    >
      <Info aria-hidden className="mt-0.5 h-4 w-4 flex-none text-muted-foreground" />
      <p className="min-w-0 text-sm text-muted-foreground">
        <span className="font-medium text-foreground">
          {share
            ? t("common.sampledLog.title", {
                count: share.denominator,
                numerator: format.number(share.numerator),
                denominator: format.number(share.denominator),
              })
            : t("common.sampledLog.noneTitle")}
        </span>{" "}
        {share ? t(`common.sampledLog.${kind}`) : t("common.sampledLog.noneBody")}
      </p>
    </div>
  );
}
