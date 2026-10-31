import { useMutation } from "@tanstack/react-query";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import { Field } from "@/components/ui/field";
import { Textarea } from "@/components/ui/textarea";
import {
  previewPayloadRedaction,
  type RedactPatternDto,
  type RedactionPreviewDto,
} from "@/lib/api";
import { errorText } from "@/lib/error-copy";

// dry-runs the payload redaction policy against a pasted sample (#1835). The
// policy sent is the one in the form, saved or not; the sample lives in this
// component's state only and is never written to a cache, a draft or the log
export function RedactionPreview({
  fields,
  detectors,
  patterns,
}: {
  fields: string[];
  detectors: string[];
  patterns: RedactPatternDto[];
}) {
  const { t } = useTranslation();
  const [sample, setSample] = React.useState("");
  const run = useMutation<RedactionPreviewDto, Error, string>({
    mutationFn: (body) =>
      previewPayloadRedaction({
        sample: body,
        redact_fields: fields,
        redact_detectors: detectors,
        redact_patterns: patterns,
      }),
  });
  const found = run.data
    ? Object.entries(run.data.report.detections).map(([kind, n]) => `${kind} ×${n}`)
    : [];
  const byField = run.data?.report.fields ?? 0;
  if (byField > 0) found.push(t("pages.logsSettings.redaction.fieldsFound", { count: byField }));

  return (
    <div className="flex w-full flex-col gap-2">
      <Field
        label={t("pages.logsSettings.redaction.sample")}
        hint={t("pages.logsSettings.redaction.previewHint")}
        className="w-full"
      >
        <Textarea
          className="min-h-[88px] w-full font-mono text-xs"
          value={sample}
          placeholder={t("pages.logsSettings.redaction.samplePlaceholder")}
          onChange={(e) => setSample(e.target.value)}
        />
      </Field>
      <div>
        <Button
          type="button"
          variant="outline"
          disabled={!sample.trim() || run.isPending}
          onClick={() => run.mutate(sample)}
        >
          {t("pages.logsSettings.redaction.run")}
        </Button>
      </div>
      {run.isError && (
        <p role="alert" className="text-sm text-destructive">
          {errorText(run.error, t)}
        </p>
      )}
      {run.data && (
        <div className="flex flex-col gap-1.5" aria-live="polite">
          <p className="text-sm text-muted-foreground">
            {found.length > 0
              ? t("pages.logsSettings.redaction.found", { kinds: found.join(", ") })
              : t("pages.logsSettings.redaction.foundNothing")}
          </p>
          <CodeBlock
            value={run.data.output}
            language="text"
            wrap
            maxHeight={240}
            label={t("pages.logsSettings.redaction.output")}
          />
        </div>
      )}
    </div>
  );
}
