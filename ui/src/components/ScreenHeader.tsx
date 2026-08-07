import { RefreshCw } from "lucide-react";
import { useQueryClient, useIsFetching } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { cn } from "@/lib/utils";

// per-screen header from the design prototype: title + subtitle on the left,
// gateway-healthy pill + refresh on the right, over the вышивка rule that
// recurs under the header on every screen. the org/project scope picker lives
// in the sidebar user menu, not here.
export function ScreenHeader({ title, subtitle }: { title: string; subtitle: string }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const isGlobalFetching = useIsFetching() > 0;
  const [isManualRefreshing, setIsManualRefreshing] = useState(false);

  const isRefreshing = isManualRefreshing || isGlobalFetching;

  async function refresh() {
    setIsManualRefreshing(true);
    try {
      await queryClient.invalidateQueries();
    } finally {
      setIsManualRefreshing(false);
    }
  }

  return (
    <>
      <header className="flex flex-none items-center gap-4 px-[22px] py-4">
        <div className="min-w-0">
          <h1 className="text-lg font-semibold tracking-tight">{title}</h1>
          <p className="mt-0.5 text-sm text-muted-foreground">{subtitle}</p>
        </div>
        <div className="ml-auto flex items-center gap-2">
          <span className="inline-flex items-center gap-[7px] rounded-full border border-[color:var(--border-subtle)] px-2.5 py-[5px] font-mono text-xs text-muted-foreground">
            <span className="rl-pulse h-[7px] w-[7px] rounded-full bg-[color:var(--status-success)]" />
            {t("shell.gatewayHealthy")}
          </span>
          <button
            type="button"
            title={isRefreshing ? t("shell.refreshing") : t("shell.refresh")}
            aria-label={isRefreshing ? t("shell.refreshingData") : t("shell.refreshData")}
            aria-busy={isRefreshing}
            disabled={isRefreshing}
            onClick={() => void refresh()}
            className="flex h-[34px] w-[34px] items-center justify-center rounded-md border border-[color:var(--border-subtle)] text-muted-foreground transition-colors hover:bg-[color:var(--surface-hover)] hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50"
          >
            <RefreshCw
              aria-hidden
              className={cn("h-4 w-4", isRefreshing && "motion-safe:animate-spin")}
            />
          </button>
        </div>
      </header>
      <div className="vyshivka-rule h-[10px] flex-none opacity-[0.28]" aria-hidden />
    </>
  );
}
