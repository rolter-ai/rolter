import { Menu, RefreshCw } from "lucide-react";
import { useQueryClient, useIsFetching, type Query } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { GATEWAY_HEALTH_KEY, useGatewayHealth, type GatewayHealth } from "@/lib/gateway-health";
import { useFormat } from "@/lib/i18n/format";
import { cn } from "@/lib/utils";

// the dot takes the status fill and the label the `-text` half of the same hue
// (the Shape-or-Glyph rule). healthy keeps the muted label the pill always had:
// it is the resting state on every screen, and the green dot already says it —
// colour is for the states that need a second look
const TONE: Record<GatewayHealth, { dot: string; label: string }> = {
  checking: { dot: "bg-[color:var(--text-subtle)]", label: "text-muted-foreground" },
  healthy: { dot: "bg-[color:var(--status-success)]", label: "text-muted-foreground" },
  degraded: {
    dot: "bg-[color:var(--status-warning)]",
    label: "text-[color:var(--status-warning-text)]",
  },
  down: {
    dot: "bg-[color:var(--status-danger)]",
    label: "text-[color:var(--status-danger-text)]",
  },
  unknown: { dot: "bg-[color:var(--text-subtle)]", label: "text-muted-foreground" },
};

// what the gateway's own `/readyz` says, polled through the `/gw` proxy (#1973).
// it breathes only while the latest check came back healthy; an answer held
// through a failed check keeps its words but goes still, and the title says
// when it arrived
function GatewayPill() {
  const { t } = useTranslation();
  const fmt = useFormat();
  const { health, live, answeredAt } = useGatewayHealth();
  const tone = TONE[health];
  const when =
    answeredAt === null
      ? null
      : t(live ? "shell.gateway.checkedAt" : "shell.gateway.lastAnswerAt", {
          time: fmt.time(answeredAt),
        });
  const detail = [t(`shell.gateway.detail.${health}`), when].filter(Boolean).join(" ");
  return (
    <span
      role="status"
      title={detail}
      className="inline-flex items-center gap-[7px] whitespace-nowrap rounded-full border border-[color:var(--border-subtle)] px-2.5 py-[5px] font-mono text-xs"
    >
      <span
        aria-hidden
        className={cn(
          "h-[7px] w-[7px] flex-none rounded-full",
          tone.dot,
          health === "healthy" && live && "rl-pulse",
        )}
      />
      <span className={tone.label}>{t(`shell.gateway.${health}`)}</span>
    </span>
  );
}

// every query but the pill's own: what the refresh button re-reads and waits on
const isScreenData = (query: Query) => query.queryKey[0] !== GATEWAY_HEALTH_KEY[0];

// per-screen header from the design prototype: title + subtitle on the left,
// gateway status pill + refresh on the right, over the вышивка rule that
// recurs under the header on every screen. the org/project scope picker lives
// in the sidebar user menu, not here.
//
// below `md` it also carries the only way back to the navigation, which is a
// drawer at that width (#959); the row wraps rather than letting the status
// pill land on top of the subtitle.
export function ScreenHeader({
  title,
  subtitle,
  onOpenNav,
}: {
  title: string;
  subtitle: string;
  onOpenNav?: () => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  // the pill polls on a clock of its own, and its background check is not the
  // screen's data refreshing: counted here, it spun and disabled this button
  // every thirty seconds on every screen
  const isGlobalFetching = useIsFetching({ predicate: isScreenData }) > 0;
  const [isManualRefreshing, setIsManualRefreshing] = useState(false);

  const isRefreshing = isManualRefreshing || isGlobalFetching;

  async function refresh() {
    setIsManualRefreshing(true);
    // the pill is asked again too, but not waited for: a gateway host that
    // drops packets would hold this button busy for the probe's whole timeout,
    // and the pill already says what it is waiting on
    void queryClient.invalidateQueries({ queryKey: GATEWAY_HEALTH_KEY });
    try {
      await queryClient.invalidateQueries({ predicate: isScreenData });
    } finally {
      setIsManualRefreshing(false);
    }
  }

  return (
    <>
      <header className="flex flex-none flex-wrap items-center gap-x-4 gap-y-2 px-[22px] py-4">
        {onOpenNav && (
          <button
            type="button"
            title={t("shell.openNav")}
            aria-label={t("shell.openNav")}
            onClick={onOpenNav}
            className="-ml-1.5 flex h-10 w-10 flex-none items-center justify-center rounded-md border border-[color:var(--border-subtle)] text-muted-foreground transition-colors hover:bg-[color:var(--surface-hover)] hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring md:hidden"
          >
            <Menu aria-hidden className="h-[18px] w-[18px]" />
          </button>
        )}
        <div className="min-w-0 flex-1">
          <h1 className="text-lg font-semibold tracking-tight">{title}</h1>
          <p className="mt-0.5 text-sm text-muted-foreground">{subtitle}</p>
        </div>
        {/* below `sm` the actions take a row of their own: sharing one with a
            title that has nowhere left to go is what put the status pill on
            top of the subtitle (#959) */}
        <div className="ml-auto flex w-full items-center justify-end gap-2 sm:w-auto">
          <GatewayPill />
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
