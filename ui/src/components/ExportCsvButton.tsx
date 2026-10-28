import { useMutation } from "@tanstack/react-query";
import { Download } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import type { AnalyticsExportFile } from "@/lib/api";
import { downloadBlob } from "@/lib/audit-export";
import { errorText } from "@/lib/error-copy";
import { useToast } from "@/lib/toast";

/**
 * The Export button of a screen whose figures end in a spreadsheet (#1838).
 *
 * `run` asks the control plane for the file, which it builds under the caller's
 * own row scope and names after its window and currency; the button only hands
 * it to the browser. A failure is a toast, since the screen has nowhere else
 * for it, and a file never starts downloading half built.
 */
export function ExportCsvButton({
  run,
  fallbackName,
  what,
  disabled,
}: {
  run: () => Promise<AnalyticsExportFile>;
  /** used only if the server sent no file name */
  fallbackName: string;
  /** the translated noun a failure names, such as "request log export" */
  what: string;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const toast = useToast();
  const exportFile = useMutation({
    mutationFn: run,
    onSuccess: (file) => {
      downloadBlob(file.name ?? fallbackName, file.blob);
      toast.push({ tone: "success", title: t("analytics.export.downloaded") });
    },
    onError: (error) =>
      toast.push({
        tone: "error",
        title: t("toast.exportFailed", { what }),
        detail: errorText(error, t),
      }),
  });
  return (
    <Button
      size="sm"
      variant="outline"
      disabled={disabled || exportFile.isPending}
      onClick={() => exportFile.mutate()}
    >
      <Download className="h-3.5 w-3.5" />
      {exportFile.isPending ? t("analytics.export.pending") : t("analytics.export.csv")}
    </Button>
  );
}
