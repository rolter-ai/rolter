import { useMutation } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { GatedButton } from "@/components/GatedButton";
import { Badge } from "@/components/ui/badge";
import { CodeBlock } from "@/components/ui/code-block";
import { Field } from "@/components/ui/field";
import { SwitchRow } from "@/components/ui/switch-row";
import { Textarea } from "@/components/ui/textarea";
import {
  testGuardrailRule,
  type GuardrailRuleInput,
  type GuardrailRuleTestInput,
  type GuardrailRuleTestResult,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";

const OUTCOME_COPY = {
  no_match: "pages.guardrailRules.trial.outcome.noMatch",
  annotated: "pages.guardrailRules.trial.outcome.annotated",
  redacted: "pages.guardrailRules.trial.outcome.redacted",
  blocked: "pages.guardrailRules.trial.outcome.blocked",
  skipped: "pages.guardrailRules.trial.outcome.skipped",
} as const;

const SKIP_COPY = {
  system_message: "pages.guardrailRules.trial.skip.systemMessage",
  over_scan_budget: "pages.guardrailRules.trial.skip.overScanBudget",
} as const;

const TONE = {
  no_match: "neutral",
  annotated: "info",
  redacted: "warning",
  blocked: "danger",
  skipped: "outline",
} as const;

/** the sample cut into the text between matches and the matches themselves */
function cut(sample: string, spans: GuardrailRuleTestResult["spans"]) {
  const parts: { text: string; hit: boolean }[] = [];
  let at = 0;
  for (const span of spans) {
    if (span.start > at) parts.push({ text: sample.slice(at, span.start), hit: false });
    parts.push({ text: sample.slice(span.start, span.end), hit: true });
    at = span.end;
  }
  if (at < sample.length) parts.push({ text: sample.slice(at), hit: false });
  return parts;
}

/**
 * Run the rule being edited against a sample before it is published (#2159).
 *
 * The control plane compiles the draft with the gateway's own matcher, so a
 * pattern the gateway would refuse is refused here with its message, and the
 * answer is the one live traffic would get. Nothing is saved, and the sample
 * stays in this component: it is sent to the test endpoint and nowhere else.
 */
export function GuardrailRuleTrial({ draft }: { draft: GuardrailRuleInput }) {
  const { t } = useTranslation();
  const [sample, setSample] = React.useState("");
  const [system, setSystem] = React.useState(false);
  const run = useMutation({ mutationFn: testGuardrailRule });

  const body: GuardrailRuleTestInput = {
    builtin: draft.source_type === "builtin" ? draft.builtin : null,
    pattern: draft.source_type === "pattern" ? draft.pattern : null,
    stage: draft.stage,
    action: draft.action,
    replacement: draft.replacement,
    include_system: draft.include_system,
    system,
    sample,
  };
  const ready =
    sample !== "" && (draft.source_type === "builtin" || Boolean(draft.pattern?.trim()));
  // an answer belongs to the draft and sample it was asked about; once either
  // changes it describes something that is no longer on screen
  const current =
    run.data && JSON.stringify(run.variables) === JSON.stringify(body) ? run.data : undefined;
  const failed =
    run.isError && JSON.stringify(run.variables) === JSON.stringify(body) ? run.error : undefined;

  return (
    <div className="space-y-3">
      <p className="text-xs text-muted-foreground">{t("pages.guardrailRules.trial.intro")}</p>
      <Field label={t("pages.guardrailRules.trial.fieldSample")} htmlFor="rule-trial-sample">
        <Textarea
          id="rule-trial-sample"
          rows={4}
          value={sample}
          placeholder={t("pages.guardrailRules.trial.samplePlaceholder")}
          onChange={(event) => setSample(event.target.value)}
        />
      </Field>
      <SwitchRow
        title={t("pages.guardrailRules.trial.system")}
        hint={t("pages.guardrailRules.trial.systemHint")}
        checked={system}
        onChange={setSystem}
      />
      <div className="flex items-center gap-3">
        <GatedButton
          gate="guardrail_rule:update"
          control="guardrail-rule-trial"
          variant="outline"
          disabled={!ready || run.isPending}
          onClick={() => run.mutate(body)}
        >
          {run.isPending && <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" />}
          {t("pages.guardrailRules.trial.run")}
        </GatedButton>
      </div>
      {failed && (
        <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
          {errorText(failed, t)}
        </p>
      )}
      {current && (
        <div role="status" className="space-y-2 text-sm">
          <p className="flex flex-wrap items-center gap-2">
            <Badge tone={TONE[current.outcome]}>{t(OUTCOME_COPY[current.outcome])}</Badge>
            <span>
              {current.skip
                ? t(SKIP_COPY[current.skip])
                : t("pages.guardrailRules.trial.matches", { count: current.match_count })}
            </span>
          </p>
          {current.spans.length > 0 && (
            <p
              aria-label={t("pages.guardrailRules.trial.highlightLabel")}
              className="whitespace-pre-wrap break-words rounded-md border border-[color:var(--border-subtle)] p-2 font-mono text-xs"
            >
              {cut(sample, current.spans).map((part, index) =>
                part.hit ? (
                  <mark
                    key={index}
                    className="rounded-sm bg-[color:var(--status-warning)]/25 text-foreground"
                  >
                    {part.text}
                  </mark>
                ) : (
                  <React.Fragment key={index}>{part.text}</React.Fragment>
                ),
              )}
            </p>
          )}
          {current.redacted !== undefined && (
            <CodeBlock
              value={current.redacted}
              label={t("pages.guardrailRules.trial.redactedLabel")}
              wrap
              copy={false}
            />
          )}
          {current.match_count > current.spans.length && (
            <p className="text-xs text-muted-foreground">
              {t("pages.guardrailRules.trial.spansCapped", { shown: current.spans.length })}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
