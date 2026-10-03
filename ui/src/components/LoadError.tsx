import {
  DatabaseZap,
  KeyRound,
  PlugZap,
  RefreshCw,
  ServerCrash,
  ShieldAlert,
  ShieldX,
} from "lucide-react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { classifyLoadError, isRetryable, needsSignIn, type LoadErrorKind } from "@/lib/load-error";
import { useOptionalAuth } from "@/lib/auth";
import { useErrorState } from "@/lib/ux-react";

const ICONS: Record<LoadErrorKind, typeof KeyRound> = {
  unauthenticated: KeyRound,
  forbidden: ShieldX,
  openMode: ShieldAlert,
  noStore: DatabaseZap,
  unreachable: PlugZap,
  server: ServerCrash,
  unknown: ServerCrash,
};

/**
 * Why a screen could not load, and what to do about it (#962).
 *
 * Replaces "Failed to load X." — one sentence that covered five causes needing
 * five different responses, and so pointed at none of them. It names the cause,
 * offers the action that can actually fix it, and shows the control plane's own
 * message underneath rather than swallowing it.
 *
 * An empty result is not a failure and must not reach this component: render an
 * empty state for that.
 *
 * It records the `error_state` UX event itself (#2444), once each time it
 * appears, the way `EmptyState` records `empty_state`. A screen does not call
 * `useErrorState` beside it: the placeholder on screen is the signal, so a
 * surface that renders one cannot forget to report it.
 */
export function LoadError({
  error,
  /** what failed to load, already translated — e.g. "virtual keys" */
  resource,
  /** re-runs the query; omit when the caller has no handle to retry with */
  onRetry,
  /**
   * stable name of the region that failed (`virtual-keys`, `request-logs`),
   * recorded on the `error_state` UX event. required, so a new call site cannot
   * be silently unnamed; name it like the region's `EmptyState` `uxTarget` so
   * the two pair up in the dead-states query
   */
  target,
}: {
  error: unknown;
  resource: string;
  onRetry?: () => void;
  target: string;
}) {
  const { t } = useTranslation();
  // mounted only while the read is in error, so every mount is one transition
  // into the error state. no-ops outside a UxScreenProvider (Storybook, tests)
  useErrorState(true, target);
  const auth = useOptionalAuth();
  const kind = classifyLoadError(error);
  const Icon = ICONS[kind];
  const detail = error instanceof Error ? error.message : null;

  return (
    // an opaque tint, not --red-tint: this lands inside panels as often as on
    // the page, and a translucent one let a --surface-subtle band drag the
    // detail line under AA (#1725)
    <div
      role="alert"
      className="flex items-start gap-3 rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--red-tint-opaque)] px-4 py-3.5"
    >
      <Icon
        aria-hidden
        className="mt-0.5 h-4 w-4 flex-none text-[color:var(--status-danger-text)]"
      />
      <div className="flex min-w-0 flex-col gap-2">
        <p className="text-sm font-medium text-foreground">
          {t(`errors.load.${kind}.title`, { resource })}
        </p>
        {/* the body carries {{resource}} in five of the eight kinds, so it
            needs the same interpolation the title gets — without it the reader
            saw the raw placeholder on screen (#1362) */}
        <p className="text-sm text-muted-foreground">
          {t(`errors.load.${kind}.body`, { resource })}
        </p>
        {detail && (
          // the control plane's own words. The whole point of #962 is that the
          // dashboard's summary was the only thing on screen and it was wrong
          <p className="break-words font-mono text-xs text-[color:var(--text-subtle)]">{detail}</p>
        )}
        <div className="flex flex-wrap gap-2">
          {isRetryable(kind) && onRetry && (
            <Button size="sm" variant="outline" onClick={onRetry}>
              <RefreshCw aria-hidden className="mr-1.5 h-3.5 w-3.5" />
              {t("errors.load.retry")}
            </Button>
          )}
          {needsSignIn(kind) && auth && (
            <Button size="sm" variant="outline" onClick={auth.signOut}>
              {t("errors.load.signIn")}
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
