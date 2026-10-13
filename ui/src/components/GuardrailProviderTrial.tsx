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
  testGuardrailProvider,
  type GuardrailProviderInput,
  type GuardrailProviderTestErrorKind,
  type GuardrailProviderTestInput,
  type GuardrailProviderTestResult,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";

const DECISION_COPY = {
  allow: "pages.guardrailProviders.trial.decision.allow",
  block: "pages.guardrailProviders.trial.decision.block",
  transform: "pages.guardrailProviders.trial.decision.transform",
  annotate: "pages.guardrailProviders.trial.decision.annotate",
} as const;

const DECISION_TONE = {
  allow: "success",
  block: "danger",
  transform: "warning",
  annotate: "info",
} as const;

const ERROR_COPY: Record<GuardrailProviderTestErrorKind, string> = {
  egress_denied: "pages.guardrailProviders.trial.error.egressDenied",
  credential_unset: "pages.guardrailProviders.trial.error.credentialUnset",
  timeout: "pages.guardrailProviders.trial.error.timeout",
  connection: "pages.guardrailProviders.trial.error.connection",
  tls: "pages.guardrailProviders.trial.error.tls",
  http_status: "pages.guardrailProviders.trial.error.httpStatus",
  unparsable: "pages.guardrailProviders.trial.error.unparsable",
  response_too_large: "pages.guardrailProviders.trial.error.responseTooLarge",
  other: "pages.guardrailProviders.trial.error.other",
};

const CREDENTIAL_COPY = {
  not_configured: "pages.guardrailProviders.trial.credential.notConfigured",
  sent: "pages.guardrailProviders.trial.credential.sent",
  skipped: "pages.guardrailProviders.trial.credential.skipped",
} as const;

/**
 * What the gateway does with a failure of this kind. A transport failure is
 * settled by the provider's failure policy; an answer that is not a decision
 * is read as allow whatever the policy says; the two failures that happen
 * before anything is sent say nothing about live traffic.
 */
function liveEffect(
  kind: GuardrailProviderTestErrorKind,
  failureMode: GuardrailProviderInput["failure_mode"],
): string | null {
  if (kind === "credential_unset") return null;
  if (kind === "unparsable") return "pages.guardrailProviders.trial.effect.allow";
  return failureMode === "fail_open"
    ? "pages.guardrailProviders.trial.effect.failOpen"
    : "pages.guardrailProviders.trial.effect.failClosed";
}

/**
 * Send the provider being edited one sample envelope before it takes over
 * enforcement (#2265).
 *
 * The control plane makes the call, through the same egress policy it applies
 * to every other operator-supplied address, so a credential variable that only
 * the gateways have is reported as unset here rather than as a rejection.
 */
export function GuardrailProviderTrial({ draft }: { draft: GuardrailProviderInput }) {
  const { t } = useTranslation();
  const [sample, setSample] = React.useState("");
  const [skipCredential, setSkipCredential] = React.useState(false);
  const run = useMutation({ mutationFn: testGuardrailProvider });

  const withCredential = draft.auth_kind !== "none";
  const body: GuardrailProviderTestInput = {
    url: draft.url,
    stage: draft.stage,
    timeout_ms: draft.timeout_ms,
    max_body_bytes: draft.max_body_bytes,
    auth_kind: draft.auth_kind,
    auth_env: draft.auth_env,
    ...(sample.trim() !== "" ? { sample } : {}),
    ...(withCredential && skipCredential ? { skip_credential: true } : {}),
  };
  const ready =
    /^https?:\/\//.test(draft.url) &&
    draft.timeout_ms > 0 &&
    draft.max_body_bytes > 0 &&
    (!withCredential || Boolean(draft.auth_env?.trim()));
  const same = JSON.stringify(run.variables) === JSON.stringify(body);
  const current = same ? run.data : undefined;
  const failed = same && run.isError ? run.error : undefined;

  return (
    <div className="space-y-3">
      <p className="text-xs text-muted-foreground">{t("pages.guardrailProviders.trial.intro")}</p>
      <Field
        label={t("pages.guardrailProviders.trial.fieldSample")}
        htmlFor="provider-trial-sample"
      >
        <Textarea
          id="provider-trial-sample"
          rows={3}
          value={sample}
          placeholder={t("pages.guardrailProviders.trial.samplePlaceholder")}
          onChange={(event) => setSample(event.target.value)}
        />
      </Field>
      {withCredential && (
        <SwitchRow
          title={t("pages.guardrailProviders.trial.skipCredential")}
          hint={t("pages.guardrailProviders.trial.skipCredentialHint")}
          checked={skipCredential}
          onChange={setSkipCredential}
        />
      )}
      <div className="flex items-center gap-3">
        <GatedButton
          gate="guardrail_provider:update"
          control="guardrail-provider-trial"
          variant="outline"
          disabled={!ready || run.isPending}
          onClick={() => run.mutate(body)}
        >
          {run.isPending && <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" />}
          {t("pages.guardrailProviders.trial.run")}
        </GatedButton>
      </div>
      {failed && (
        <p role="alert" className="text-xs text-[color:var(--status-danger-text)]">
          {errorText(failed, t)}
        </p>
      )}
      {current && <TrialResult result={current} failureMode={draft.failure_mode} />}
    </div>
  );
}

function TrialResult({
  result,
  failureMode,
}: {
  result: GuardrailProviderTestResult;
  failureMode: GuardrailProviderInput["failure_mode"];
}) {
  const { t } = useTranslation();
  const decision = result.decision;
  const effect = result.error ? liveEffect(result.error.kind, failureMode) : null;
  return (
    <div role="status" className="space-y-2 text-sm">
      <p className="flex flex-wrap items-center gap-2">
        {decision ? (
          <Badge tone={DECISION_TONE[decision.action]}>{t(DECISION_COPY[decision.action])}</Badge>
        ) : (
          result.error && (
            <Badge tone="danger">{t("pages.guardrailProviders.trial.failedBadge")}</Badge>
          )
        )}
        {result.latency_ms > 0 && (
          <span className="text-muted-foreground">
            {t("pages.guardrailProviders.trial.latency", { ms: result.latency_ms })}
          </span>
        )}
      </p>
      {decision?.reason && (
        <p>{t("pages.guardrailProviders.trial.reason", { reason: decision.reason })}</p>
      )}
      {result.error && (
        <>
          <p className="font-medium">{t(ERROR_COPY[result.error.kind])}</p>
          {/* raw-error-allow: a result the test endpoint composes from fixed text, never a failed call or the webhook's own body */}
          <p className="text-muted-foreground">{result.error.message}</p>
          {effect && <p className="text-xs text-muted-foreground">{t(effect)}</p>}
        </>
      )}
      {decision?.content !== undefined && (
        <CodeBlock
          value={JSON.stringify(decision.content, null, 2)}
          language="json"
          label={t("pages.guardrailProviders.trial.contentLabel")}
          copy={false}
        />
      )}
      {decision?.annotations !== undefined && (
        <CodeBlock
          value={JSON.stringify(decision.annotations, null, 2)}
          language="json"
          label={t("pages.guardrailProviders.trial.annotationsLabel")}
          copy={false}
        />
      )}
      {/* an unset variable is the error above; saying it twice reads as two problems */}
      {(result.credential !== "unset" || result.truncated) && (
        <p className="text-xs text-muted-foreground">
          {result.credential !== "unset" && t(CREDENTIAL_COPY[result.credential])}
          {result.truncated && ` ${t("pages.guardrailProviders.trial.truncated")}`}
        </p>
      )}
    </div>
  );
}
