import type * as React from "react";
import { ShieldCheck } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Badge } from "@/components/ui/badge";
import { CardGridSkeleton } from "@/components/LoadingState";
import { EmptyState } from "@/components/ui/empty-state";

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
  badges,
  details,
  actions,
}: {
  title: string;
  description: string;
  enabled: boolean;
  badges: React.ReactNode;
  details: React.ReactNode;
  actions: React.ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <article className="flex min-h-[172px] flex-col rounded-[10px] border border-[color:var(--border-subtle)] bg-[color:var(--surface-raised)] p-4 transition-colors hover:border-[color:var(--border-default)]">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="truncate text-sm font-semibold">{title}</h2>
            <Badge tone={enabled ? "success" : "neutral"} dot>
              {enabled ? t("guardrailPanel.enforced") : t("guardrailPanel.paused")}
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
