import { ExternalLink } from "lucide-react";
import { useTranslation } from "react-i18next";

import { docsUrl, DOCS_PAGES, type DocsPage } from "@/lib/docs";
import type { RolterRuntimeConfig } from "@/lib/telemetry";
import { cn } from "@/lib/utils";

/**
 * A "read more" link into the user documentation, or nothing (#1164).
 *
 * Takes a page *key*, never a URL: the path lives once in `DOCS_PAGES` and the
 * hostname lives once in the deployment's configuration, so no screen ever
 * spells out either.
 *
 * Renders `null` when the deployment configured no documentation base URL.
 * That is the air-gapped case and the default: an operator with no
 * documentation host gets no link at all, rather than one that dead-ends. It
 * is also why the surrounding copy must stand on its own — this link adds
 * depth, it never carries the explanation.
 *
 * `config`/`fallback` exist for the tests and stories, which need to drive the
 * configured and unconfigured cases without writing to `window`.
 */
export function DocsLink({
  page,
  label,
  className,
  config,
  fallback,
}: {
  page: DocsPage;
  /** already-translated link text; defaults to the generic "documentation" */
  label?: string;
  className?: string;
  config?: RolterRuntimeConfig;
  fallback?: string;
}) {
  const { t } = useTranslation();
  const href = docsUrl(DOCS_PAGES[page], config, fallback);
  if (!href) return null;
  const text = label ?? t("docs.link.default");
  return (
    <a
      href={href}
      target="_blank"
      // the documentation host is a third party as far as the dashboard is
      // concerned, and it learns nothing about this deployment from a referrer
      rel="noreferrer"
      // the destination leaves the dashboard; say so where the text cannot
      aria-label={t("docs.link.opensInNewTab", { label: text })}
      className={cn(
        "inline-flex items-center gap-1 rounded-sm text-[color:var(--text-subtle)] underline decoration-dotted underline-offset-2 transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
        className,
      )}
    >
      {text}
      <ExternalLink aria-hidden="true" className="h-3 w-3 flex-none" />
    </a>
  );
}
