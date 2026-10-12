import { useTranslation } from "react-i18next";
import { Link } from "react-router";

import { DocsLink } from "@/components/DocsLink";
import { strategyDocsPage, strategyHintKey, strategyPolicyPath } from "@/lib/strategies";

/**
 * The caveat attached to a balancing strategy, or nothing (#897).
 *
 * Some strategies are not simply "pick one and it works": two need a telemetry
 * source configured on the member providers and degrade quietly to least-load
 * without it, and one is governed by a deployment-wide policy rather than by
 * this control. Selecting them without knowing that produces a route that looks
 * configured and does not behave as chosen — which is exactly the silence #897
 * was about, in a different place.
 *
 * The telemetry caveat also says where that source is set (the provider sheet
 * or `rolter.toml`) and links the cache-aware routing page when the deployment
 * has a documentation host.
 */
export function StrategyHint({ strategy }: { strategy: string }) {
  const { t } = useTranslation();
  const key = strategyHintKey(strategy);
  if (!key) return null;
  const docs = strategyDocsPage(strategy);
  const policy = strategyPolicyPath(strategy);
  return (
    <p className="mt-1.5 text-xs text-[color:var(--status-warning-text)]" role="note">
      {t(key)}
      {policy && (
        <>
          {" "}
          <Link to={policy} className="underline underline-offset-2">
            {t("pages.routing.strategyHints.policyLink")}
          </Link>
        </>
      )}
      {docs && (
        <>
          {" "}
          <DocsLink page={docs} label={t(`docs.link.${docs}`)} />
        </>
      )}
    </p>
  );
}
