import { AlertTriangle, CheckCircle2, XCircle } from "lucide-react";
import { useTranslation } from "react-i18next";

import type { ProviderTestResult } from "@/lib/api";

/**
 * Which of the probe's three outcomes a result is.
 *
 * The API reports "answered, but not a model list" as `reachable: false` with a
 * 2xx status (#980). Collapsing that into the same red as a refused connection
 * is accurate and useless: the host is up and it is the URL or the service
 * behind it that is wrong, which is a different next action from "could not
 * connect" or "the upstream refused us" (#1034).
 */
export function probeOutcome(result: ProviderTestResult): "ok" | "answered" | "failed" {
  if (result.reachable) return "ok";
  const status = result.status ?? 0;
  return status >= 200 && status < 300 ? "answered" : "failed";
}

export const OUTCOME_STYLES: Record<ReturnType<typeof probeOutcome>, string> = {
  ok: "border-[color:var(--status-success)]/40 bg-[color:var(--green-tint)]",
  answered: "border-[color:var(--status-warning)]/40 bg-[color:var(--status-warning)]/10",
  failed: "border-[color:var(--status-danger)]/40 bg-destructive/10",
};

/**
 * The result of a connection probe.
 *
 * Always names the URL that was tried. "It failed" is not actionable when the
 * operator cannot see how their `api_base` was turned into an endpoint — a
 * doubled `/v1` is the single most common cause and is invisible otherwise.
 */
export function TestOutcome({
  result,
  className = "mx-[22px] mt-2.5",
}: {
  result: ProviderTestResult;
  /** where the box sits: the provider sheet's footer gutter by default */
  className?: string;
}) {
  const { t } = useTranslation();
  const outcome = probeOutcome(result);
  return (
    <div
      role="status"
      className={`${className} rounded-md border px-3 py-2 text-xs ${OUTCOME_STYLES[outcome]}`}
    >
      <div className="flex items-center gap-1.5 font-medium">
        {outcome === "ok" && (
          <CheckCircle2 className="size-3.5 text-[color:var(--status-success-text)]" />
        )}
        {outcome === "answered" && (
          <AlertTriangle className="size-3.5 text-[color:var(--status-warning-text)]" />
        )}
        {outcome === "failed" && (
          <XCircle className="size-3.5 text-[color:var(--status-danger-text)]" />
        )}
        <span>
          {outcome === "ok"
            ? t("providerSheet.testOk", {
                count: result.models_found ?? 0,
                ms: result.latency_ms,
              })
            : outcome === "answered"
              ? t("providerSheet.testAnswered")
              : t("providerSheet.testFailed")}
        </span>
      </div>
      {result.error && <p className="mt-1 text-muted-foreground">{result.error}</p>}
      {result.probed_url && (
        <p className="mt-1 font-mono text-2xs text-muted-foreground">{result.probed_url}</p>
      )}
    </div>
  );
}

/**
 * What a provider test asked about one upstream model found (#2009).
 *
 * Five outcomes, none of which reads as another: the model is listed, it is not
 * listed, the provider answered but has no catalogue to look it up in, the
 * provider answered with something that is not a catalogue, or the probe
 * failed. Everything but "listed" is a failure to confirm, so a probe that did
 * not succeed hands over to `TestOutcome` and its reason, and never renders as
 * a green line.
 */
export function ModelCheckOutcome({
  result,
  model,
  provider,
}: {
  result: ProviderTestResult;
  model: string;
  provider: string;
}) {
  const { t } = useTranslation();
  if (probeOutcome(result) !== "ok") return <TestOutcome result={result} className="mt-1" />;
  const listed = result.model_listed;
  const tone =
    listed === true
      ? OUTCOME_STYLES.ok
      : listed === false
        ? OUTCOME_STYLES.answered
        : "border-border bg-muted/40";
  return (
    <div role="status" className={`mt-1 rounded-md border px-3 py-2 text-xs ${tone}`}>
      <div className="flex items-start gap-1.5">
        {listed === true && (
          <CheckCircle2 className="mt-0.5 size-3.5 shrink-0 text-[color:var(--status-success-text)]" />
        )}
        {listed === false && (
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-[color:var(--status-warning-text)]" />
        )}
        <span className="min-w-0 break-words font-medium">
          {listed === true
            ? t("modelSheet.check.listed", { provider, model })
            : listed === false
              ? t("modelSheet.check.notListed", { provider, model })
              : t("modelSheet.check.noCatalogue", { provider, model })}
        </span>
      </div>
    </div>
  );
}
