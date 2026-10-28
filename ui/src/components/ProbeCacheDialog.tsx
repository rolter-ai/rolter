import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { probeProviderCache, type ProviderRow } from "@/lib/api";
import { errorText } from "@/lib/error-copy";
import { PROVIDER_CACHE_PROBES_KEY } from "@/lib/use-provider-cache-probes";
import { useFormTelemetry } from "@/lib/ux-react";

/**
 * Ask a provider for one token and record whether its answer reports
 * prompt-cache usage. The probe is a real completion, so it is never run
 * unprompted: the operator names the upstream model and presses the button.
 */
export function ProbeCacheDialog({
  provider,
  suggestedModel,
  onOpenChange,
}: {
  provider: ProviderRow | null;
  /** the model the last verdict used, to probe again with the same one */
  suggestedModel?: string;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const open = !!provider;
  const [model, setModel] = React.useState("");

  const probe = useMutation({
    mutationFn: (input: { id: string; model: string }) => probeProviderCache(input.id, input.model),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: [PROVIDER_CACHE_PROBES_KEY, provider?.org_id] }),
  });
  const reset = probe.reset;

  // the suggestion is read when the dialog opens for a provider and not
  // followed afterwards: a finished probe refreshes the verdicts it comes from,
  // and a reset on that would wipe the result it just produced
  const suggestion = React.useRef(suggestedModel);
  suggestion.current = suggestedModel;
  React.useEffect(() => {
    if (open) {
      setModel(suggestion.current ?? "");
      reset();
    }
  }, [open, provider?.id, reset]);

  const ux = useFormTelemetry("provider-probe-cache", open, { dirty: model.trim() !== "" });
  const result = probe.data;
  const verdict = result?.verdict;

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      onSubmit={() => {
        if (!provider) return;
        ux.submitted();
        probe.mutate(
          { id: provider.id, model: model.trim() },
          { onSuccess: () => ux.saved(), onError: () => ux.failed() },
        );
      }}
      canSubmit={!!model.trim()}
      busy={probe.isPending}
    >
      <DialogHeader>
        <DialogTitle>
          {t("pages.providers.probe.title", { name: provider?.name ?? "" })}
        </DialogTitle>
        <DialogDescription>{t("pages.providers.probe.body")}</DialogDescription>
      </DialogHeader>
      <DialogBody>
        <div className="space-y-3">
          <Field
            label={t("pages.providers.probe.model")}
            hint={t("pages.providers.probe.modelHint")}
            required
          >
            <Input
              className="font-mono"
              value={model}
              onChange={(e) => setModel(e.target.value)}
              placeholder={t("pages.providers.probe.modelPlaceholder")}
            />
          </Field>
          <div aria-live="polite" className="space-y-1 text-xs">
            {result && (
              <>
                <p
                  role={result.recorded ? "status" : "alert"}
                  className={
                    result.recorded
                      ? "text-[color:var(--text-secondary)]"
                      : "text-[color:var(--status-warning-text)]"
                  }
                >
                  {!result.recorded
                    ? t("pages.providers.probe.notRecorded", {
                        error: result.error ?? t("pages.providers.probe.noReason"),
                      })
                    : verdict?.reports
                      ? t("pages.providers.probe.recordedReports", {
                          spelling: verdict.spelling ?? "",
                        })
                      : t("pages.providers.probe.recordedNone")}
                </p>
                <p className="font-mono text-muted-foreground">
                  {result.status === null
                    ? t("pages.providers.probe.noAnswer", {
                        url: result.probed_url,
                        ms: result.latency_ms,
                      })
                    : t("pages.providers.probe.answered", {
                        url: result.probed_url,
                        status: result.status,
                        ms: result.latency_ms,
                      })}
                </p>
              </>
            )}
            {probe.isError && (
              <p role="alert" className="text-[color:var(--status-danger-text)]">
                {errorText(probe.error, t)}
              </p>
            )}
          </div>
        </div>
      </DialogBody>
      <DialogFooter>
        <Button variant="outline" onClick={() => onOpenChange(false)}>
          {t("common.close")}
        </Button>
        <Button
          type="submit"
          disabled={probe.isPending}
          aria-disabled={!model.trim() || undefined}
          className="aria-disabled:opacity-50"
        >
          {probe.isPending && <Loader2 className="mr-2 h-4 w-4 motion-safe:animate-spin" />}
          {t("pages.providers.probe.run")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
