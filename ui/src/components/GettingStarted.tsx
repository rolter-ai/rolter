import { useQuery } from "@tanstack/react-query";
import { ArrowRight, Check, Compass, Rocket, X } from "lucide-react";
import * as React from "react";
import { Link } from "react-router";
import { useTranslation } from "react-i18next";

import { GatedButton } from "@/components/GatedButton";
import { LoadError } from "@/components/LoadError";
import { LoadingRegion } from "@/components/LoadingState";
import { Button, buttonVariants } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { CodeBlock } from "@/components/ui/code-block";
import { EmptyState } from "@/components/ui/empty-state";
import { Skeleton } from "@/components/ui/skeleton";
import { fetchProviders, fetchRoutes, fetchVirtualKeys } from "@/lib/api";
import { useGate, type Capability } from "@/lib/can";
import { gatewayBaseUrl } from "@/lib/gateway";
import { useScope } from "@/lib/scope";
import { cn } from "@/lib/utils";

// The thread between the screens a first-run admin has to visit in order
// (#1585). Every one of those screens already has a correct empty state with a
// CTA; what was missing is that nothing said which of them comes first, so the
// zero-credential start — `rolter easy-up` plus the built-in `fake-llm` — was
// only discoverable from the documentation.
//
// It is a component on the dashboard rather than a screen of its own: a screen
// would need a nav entry that outlives its usefulness, and the point of this
// surface is that it goes away.

const DISMISSED_KEY = "rolter.getting-started.dismissed";

function readDismissed(): boolean {
  try {
    return localStorage.getItem(DISMISSED_KEY) === "1";
  } catch {
    // no storage (private window, blocked site data) — the card just comes back
    return false;
  }
}

function writeDismissed(value: boolean): void {
  try {
    if (value) localStorage.setItem(DISMISSED_KEY, "1");
    else localStorage.removeItem(DISMISSED_KEY);
  } catch {
    // same: dismissal is a convenience, not state anything depends on
  }
}

interface Step {
  key: string;
  done: boolean;
  to: string;
  /** the capability the CTA's destination will ask for, when it creates a row */
  gate?: Capability;
}

/** one step: a state marker, the copy, and the link that does it */
function StepRow({ step, index }: { step: Step; index: number }) {
  const { t } = useTranslation();
  // use-gate-allow: picks a link or a button; the refused button is a
  // `GatedButton`, which is what records the reach for it
  const { denied } = useGate(step.gate);
  const label = t(`pages.gettingStarted.steps.${step.key}.title`);
  return (
    <li className="flex items-start gap-3">
      <span
        aria-hidden
        className={cn(
          "mt-0.5 flex h-6 w-6 flex-none items-center justify-center rounded-full border text-xs",
          step.done
            ? "border-transparent bg-[color:var(--red-folk)] text-white"
            : "border-[color:var(--border-default)] text-muted-foreground",
        )}
      >
        {step.done ? <Check className="h-3.5 w-3.5" /> : index + 1}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm">{label}</span>
          <span
            className="text-xs text-muted-foreground"
            data-testid={`getting-started-state-${step.key}`}
          >
            {step.done ? t("pages.gettingStarted.done") : t("pages.gettingStarted.todo")}
          </span>
        </div>
        <p className="mt-0.5 text-xs leading-snug text-muted-foreground">
          {t(`pages.gettingStarted.steps.${step.key}.body`)}
        </p>
        <div className="mt-2">
          {denied && step.gate ? (
            // the house pattern for a control the caller may not use (#1183):
            // present, disabled, and saying which role it takes — rather than
            // a link that spends the operator's attention on a 403. the
            // capability tells the steps apart in `refused_click`, so they
            // share one slug
            <GatedButton
              gate={step.gate}
              control="getting-started-step"
              size="sm"
              variant="outline"
            >
              {t(`pages.gettingStarted.steps.${step.key}.action`)}
            </GatedButton>
          ) : (
            <Link
              to={step.to}
              className={cn(buttonVariants({ variant: "outline", size: "sm" }), "gap-1.5")}
            >
              {t(`pages.gettingStarted.steps.${step.key}.action`)}
              <ArrowRight className="h-3.5 w-3.5" />
            </Link>
          )}
        </div>
      </div>
    </li>
  );
}

export interface GettingStartedProps {
  /**
   * Requests in the dashboard's window. `undefined` where the deployment has no
   * analytics store to ask, which is not the same as a quiet day — the card
   * stays in that case, since the steps below still reflect real rows.
   */
  requests?: number;
}

/**
 * The first-run checklist, or nothing at all.
 *
 * Each step is done when the resource exists, not when somebody ticked a box:
 * reloading, or another admin doing the work, updates it. The card retires
 * itself once the deployment has both traffic and a provider — traffic alone
 * can be one `fake-llm` call in the Playground, which is step one of five.
 */
export function GettingStarted({ requests }: GettingStartedProps) {
  const { t } = useTranslation();
  const scope = useScope();
  const [dismissed, setDismissed] = React.useState(readDismissed);

  const providers = useQuery({
    queryKey: ["providers", scope.orgId],
    queryFn: () => fetchProviders(scope.orgId as string),
    enabled: !!scope.orgId,
    retry: false,
  });
  const routes = useQuery({
    queryKey: ["routes", scope.projectId],
    queryFn: () => fetchRoutes(scope.projectId as string),
    enabled: !!scope.projectId,
    retry: false,
  });
  const keys = useQuery({
    queryKey: ["virtual-keys", scope.projectId],
    queryFn: () => fetchVirtualKeys(scope.projectId as string),
    enabled: !!scope.projectId,
    retry: false,
  });

  const hasProvider = (providers.data?.length ?? 0) > 0;
  const hasTraffic = (requests ?? 0) > 0;

  const dismiss = () => {
    writeDismissed(true);
    setDismissed(true);
  };
  const reopen = () => {
    writeDismissed(false);
    setDismissed(false);
  };

  // configured and serving: the card has said everything it has to say
  if (hasTraffic && hasProvider) return null;

  if (dismissed) {
    return (
      <div className="flex justify-end">
        <Button variant="ghost" size="sm" className="gap-1.5" onClick={reopen}>
          <Compass className="h-3.5 w-3.5" />
          {t("pages.gettingStarted.reopen")}
        </Button>
      </div>
    );
  }

  const failed = providers.error ?? routes.error ?? keys.error;
  const loading = scope.isLoading || providers.isLoading || routes.isLoading || keys.isLoading;

  const steps: Step[] = [
    { key: "call", done: hasTraffic, to: "/playground" },
    { key: "provider", done: hasProvider, to: "/providers", gate: "provider:create" },
    {
      key: "route",
      done: (routes.data?.length ?? 0) > 0,
      to: "/routing-rules",
      gate: "route:create",
    },
    {
      key: "key",
      done: (keys.data?.length ?? 0) > 0,
      to: "/virtual-keys",
      gate: "virtual_key:create",
    },
  ];

  const snippet = [
    `curl ${gatewayBaseUrl()}/v1/chat/completions \\`,
    `  -H "Authorization: Bearer $ROLTER_VIRTUAL_KEY" \\`,
    `  -H "Content-Type: application/json" \\`,
    `  -d '{"model": "fake-llm", "messages": [{"role": "user", "content": "hi"}]}'`,
  ].join("\n");

  return (
    <Card>
      <CardHeader className="flex-row items-start justify-between gap-3 space-y-0">
        <div className="flex items-start gap-3">
          <span className="mt-0.5 flex h-8 w-8 flex-none items-center justify-center rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] text-muted-foreground">
            <Rocket className="h-4 w-4" />
          </span>
          <div>
            <CardTitle>{t("pages.gettingStarted.title")}</CardTitle>
            <CardDescription>{t("pages.gettingStarted.subtitle")}</CardDescription>
          </div>
        </div>
        <Button
          variant="ghost"
          size="sm"
          className="gap-1.5"
          onClick={dismiss}
          aria-label={t("pages.gettingStarted.dismiss")}
        >
          <X className="h-3.5 w-3.5" />
          {t("pages.gettingStarted.dismiss")}
        </Button>
      </CardHeader>
      <CardContent>
        {loading ? (
          <LoadingRegion className="flex flex-col gap-3">
            {Array.from({ length: 4 }, (_, i) => (
              <div key={i} className="flex items-start gap-3">
                <Skeleton className="h-6 w-6 rounded-full" />
                <div className="flex-1 space-y-1.5">
                  <Skeleton className="h-3.5 w-48" />
                  <Skeleton className="h-3 w-72" />
                </div>
              </div>
            ))}
          </LoadingRegion>
        ) : failed ? (
          <LoadError
            error={failed}
            resource={t("errors.resources.gettingStarted")}
            onRetry={() => {
              void providers.refetch();
              void routes.refetch();
              void keys.refetch();
            }}
          />
        ) : !scope.projectId ? (
          // nothing below can reflect real state without a project to read it
          // from, and a checklist of four unknowns is worse than none
          <EmptyState
            uxTarget="getting-started"
            title={t("pages.gettingStarted.noScopeTitle")}
            description={t("pages.gettingStarted.noScopeBody")}
          />
        ) : (
          <div className="flex flex-col gap-5">
            <ol className="flex flex-col gap-4">
              {steps.map((step, i) => (
                <StepRow key={step.key} step={step} index={i} />
              ))}
            </ol>
            <div>
              <div className="text-sm">{t("pages.gettingStarted.steps.client.title")}</div>
              <p className="mt-0.5 mb-2 text-xs leading-snug text-muted-foreground">
                {t("pages.gettingStarted.steps.client.body")}
              </p>
              <CodeBlock
                value={snippet}
                language="bash"
                label={t("pages.gettingStarted.snippetLabel")}
                wrap
              />
            </div>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
