import { useMutation } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { GatedButton } from "@/components/GatedButton";
import { Badge } from "@/components/ui/badge";
import { CodeBlock } from "@/components/ui/code-block";
import { Field } from "@/components/ui/field";
import { FieldLabel } from "@/components/ui/field-label";
import { Segmented } from "@/components/ui/segmented";
import { SwitchRow } from "@/components/ui/switch-row";
import { Textarea } from "@/components/ui/textarea";
import {
  testGuardrailPolicy,
  type GuardrailPolicyStep,
  type GuardrailPolicyTestInput,
  type GuardrailPolicyTestResult,
  type GuardrailRuleRow,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";

const STEP_COPY = {
  no_match: "pages.guardrailRules.policyTrial.step.noMatch",
  annotated: "pages.guardrailRules.policyTrial.step.annotated",
  redacted: "pages.guardrailRules.policyTrial.step.redacted",
  blocked: "pages.guardrailRules.policyTrial.step.blocked",
  skipped: "pages.guardrailRules.policyTrial.step.skipped",
} as const;

const VERDICT_COPY = {
  no_match: "pages.guardrailRules.policyTrial.verdict.noMatch",
  annotated: "pages.guardrailRules.policyTrial.verdict.annotated",
  redacted: "pages.guardrailRules.policyTrial.verdict.redacted",
  blocked: "pages.guardrailRules.policyTrial.verdict.blocked",
  skipped: "pages.guardrailRules.policyTrial.verdict.skipped",
} as const;

const SKIP_COPY = {
  system_message: "pages.guardrailRules.trial.skip.systemMessage",
  over_scan_budget: "pages.guardrailRules.trial.skip.overScanBudget",
  other_stage: "pages.guardrailRules.policyTrial.skip.otherStage",
  after_block: "pages.guardrailRules.policyTrial.skip.afterBlock",
  policy_disabled: "pages.guardrailRules.policyTrial.skip.policyDisabled",
  not_compiled: "pages.guardrailRules.policyTrial.skip.notCompiled",
} as const;

const SOURCE_COPY = {
  file: "pages.guardrailRules.policyTrial.source.file",
  dashboard: "pages.guardrailRules.policyTrial.source.dashboard",
  draft: "pages.guardrailRules.policyTrial.source.draft",
} as const;

const TONE = {
  no_match: "neutral",
  annotated: "info",
  redacted: "warning",
  blocked: "danger",
  skipped: "outline",
} as const;

function StepDetail({ step }: { step: GuardrailPolicyStep }) {
  const { t } = useTranslation();
  if (step.skip) return <>{t(SKIP_COPY[step.skip])}</>;
  if (step.outcome === "no_match") return null;
  return <>{t("pages.guardrailRules.trial.matches", { count: step.match_count })}</>;
}

/**
 * The per-rule trace of a policy run: every rule in the order the gateway scans
 * them, what each did to the text it was handed, and the verdict. An earlier
 * redaction changes what a later rule sees, which is why the counts here can
 * differ from running the same rule alone.
 */
export function PolicyTrace({ result }: { result: GuardrailPolicyTestResult }) {
  const { t } = useTranslation();
  return (
    <div role="status" className="space-y-2 text-sm">
      <p className="flex flex-wrap items-center gap-2">
        <Badge tone={TONE[result.outcome]}>
          {t(VERDICT_COPY[result.outcome], { rule: result.blocked_by })}
        </Badge>
      </p>
      {result.steps.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          {t("pages.guardrailRules.policyTrial.noRules")}
        </p>
      ) : (
        <ol
          aria-label={t("pages.guardrailRules.policyTrial.traceLabel")}
          className="divide-y divide-[color:var(--border-subtle)] rounded-md border border-[color:var(--border-subtle)]"
        >
          {result.steps.map((step, index) => (
            <li
              key={`${step.source}-${step.rule}`}
              className="flex flex-wrap items-center gap-x-2 gap-y-1 px-3 py-2"
            >
              <span className="w-5 text-xs tabular-nums text-muted-foreground">{index + 1}</span>
              <span className="font-mono text-xs">{step.rule}</span>
              <Badge tone={step.source === "draft" ? "info" : "outline"}>
                {t(SOURCE_COPY[step.source])}
              </Badge>
              <Badge tone={TONE[step.outcome]}>{t(STEP_COPY[step.outcome])}</Badge>
              <span className="text-xs text-muted-foreground">
                <StepDetail step={step} />
              </span>
            </li>
          ))}
        </ol>
      )}
      {result.redacted !== undefined && (
        <CodeBlock
          value={result.redacted}
          label={t("pages.guardrailRules.policyTrial.finalLabel")}
          wrap
          copy={false}
        />
      )}
    </div>
  );
}

/**
 * Run the whole policy on a sample (#3084): config-file rules first, then the
 * enabled dashboard rules by position, with redactions feeding later rules and
 * the first block ending the scan. The control plane runs the gateway's own
 * scan over the rule list the gateway is served, so the answer is the one live
 * traffic would get. Nothing is saved, and the sample stays in this component:
 * it is sent to the test endpoint and nowhere else.
 */
export function GuardrailPolicyTrial() {
  const { t } = useTranslation();
  const [sample, setSample] = React.useState("");
  const [system, setSystem] = React.useState(false);
  const [stage, setStage] = React.useState<GuardrailRuleRow["stage"]>("pre_call");
  const stageLabelId = React.useId();
  const run = useMutation({ mutationFn: testGuardrailPolicy });

  const body: GuardrailPolicyTestInput = { sample, stage, system };
  const same = JSON.stringify(run.variables) === JSON.stringify(body);
  // an answer belongs to the sample it was asked about; once that changes it
  // describes something that is no longer on screen
  const current = run.data && same ? run.data : undefined;
  const failed = run.isError && same ? run.error : undefined;

  return (
    <div className="space-y-3">
      <Field label={t("pages.guardrailRules.trial.fieldSample")} htmlFor="policy-trial-sample">
        <Textarea
          id="policy-trial-sample"
          rows={5}
          value={sample}
          placeholder={t("pages.guardrailRules.trial.samplePlaceholder")}
          onChange={(event) => setSample(event.target.value)}
        />
      </Field>
      <div className="space-y-1.5">
        <FieldLabel id={stageLabelId} label={t("pages.guardrailRules.policyTrial.stage")} />
        <Segmented
          labelledBy={stageLabelId}
          value={stage}
          options={[
            { value: "pre_call", label: t("pages.guardrailRules.policyTrial.stageRequest") },
            { value: "post_call", label: t("pages.guardrailRules.policyTrial.stageResponse") },
          ]}
          onChange={setStage}
        />
      </div>
      <SwitchRow
        title={t("pages.guardrailRules.trial.system")}
        hint={t("pages.guardrailRules.trial.systemHint")}
        checked={system}
        onChange={setSystem}
      />
      <GatedButton
        gate="guardrail_rule:update"
        control="guardrail-policy-trial"
        variant="outline"
        disabled={sample === "" || run.isPending}
        onClick={() => run.mutate(body)}
      >
        {run.isPending && <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" />}
        {t("pages.guardrailRules.policyTrial.run")}
      </GatedButton>
      {failed && (
        <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
          {errorText(failed, t)}
        </p>
      )}
      {current && <PolicyTrace result={current} />}
    </div>
  );
}
