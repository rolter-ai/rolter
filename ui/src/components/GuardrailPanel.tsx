import * as React from "react";
import { ShieldCheck } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { CardGridSkeleton } from "@/components/LoadingState";
import { EmptyState } from "@/components/ui/empty-state";
import { cn } from "@/lib/utils";

/**
 * The card grid both guardrail screens stand in while their query is in flight.
 *
 * It used to be a hand-rolled `role="status"` div of bare skeletons carrying a
 * name of its own. The region was right, the name was not: every other screen
 * announces the shared `common.loading`, so a reader moving between screens
 * heard a different word for the same state, and the div carried no `aria-busy`
 * (#1605). `CardGridSkeleton` is that region, once.
 */
export function GuardrailLoading() {
  return <CardGridSkeleton cards={3} height={172} min={320} />;
}

export function GuardrailEmpty({
  title,
  description,
  action,
}: {
  title: string;
  description: string;
  action: React.ReactNode;
}) {
  return (
    <EmptyState
      uxTarget="guardrail-list"
      icon={<ShieldCheck />}
      title={title}
      description={description}
      actions={action}
    />
  );
}

export function PolicyCard({
  title,
  description,
  enabled,
  status,
  badges,
  details,
  actions,
  headingLevel = "h2",
}: {
  title: string;
  description: string;
  enabled: boolean;
  /**
   * The status badge, when `enabled` alone would overstate it. A guardrail
   * provider can be switched on and still enforce nothing (#2162).
   */
  status?: { tone: "success" | "warning" | "neutral"; label: string };
  badges: React.ReactNode;
  details: React.ReactNode;
  actions: React.ReactNode;
  /** `h3` for a card listed under a section heading of its own */
  headingLevel?: "h2" | "h3";
}) {
  const { t } = useTranslation();
  const badge = status ?? {
    tone: enabled ? ("success" as const) : ("neutral" as const),
    label: enabled ? t("guardrailPanel.enforced") : t("guardrailPanel.paused"),
  };
  const Heading = headingLevel;
  return (
    <article className="flex min-h-[172px] flex-col rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)] p-4 transition-colors hover:border-[color:var(--border-default)]">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <Heading className="truncate text-sm font-semibold">{title}</Heading>
            <Badge tone={badge.tone} dot>
              {badge.label}
            </Badge>
          </div>
          <p className="mt-1 line-clamp-2 text-sm text-muted-foreground">{description}</p>
        </div>
      </div>
      <div className="mt-3 flex flex-wrap gap-1.5">{badges}</div>
      <div className="mt-3 text-xs text-[color:var(--text-subtle)]">{details}</div>
      <div className="mt-auto flex justify-end gap-2 pt-4">{actions}</div>
    </article>
  );
}

const BANNER_TONE = {
  warning: "border-[color:var(--status-warning)]/30 bg-[color:var(--status-warning)]/5",
  neutral: "border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)]",
};

/**
 * A line above the cards saying the policy they list is not what the gateway
 * runs, and why (#2157). Named by its title, so it is a region a screen reader
 * can jump to.
 */
export function GuardrailBanner({
  tone,
  icon,
  title,
  children,
  action,
}: {
  tone: keyof typeof BANNER_TONE;
  icon: React.ReactNode;
  title: string;
  /** the body, one or more short paragraphs */
  children: React.ReactNode;
  /** a link or button on the right, for the one thing that changes the state */
  action?: React.ReactNode;
}) {
  const titleId = React.useId();
  return (
    <section
      aria-labelledby={titleId}
      className={cn(
        "flex flex-col gap-3 rounded-[10px] border p-4 sm:flex-row sm:items-start",
        BANNER_TONE[tone],
      )}
    >
      <div className="flex min-w-0 flex-1 items-start gap-3">
        <span className="mt-0.5 flex-none">{icon}</span>
        <div className="min-w-0 flex-1 space-y-0.5">
          <p id={titleId} className="text-sm font-medium">
            {title}
          </p>
          <div className="space-y-0.5 break-words text-xs text-muted-foreground">{children}</div>
        </div>
      </div>
      {action && <div className="flex-none pl-8 sm:pl-0">{action}</div>}
    </section>
  );
}
