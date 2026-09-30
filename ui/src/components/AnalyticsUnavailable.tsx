import { ChartNoAxesColumn } from "lucide-react";
import { Trans, useTranslation } from "react-i18next";

/**
 * What a screen shows on a deployment with no analytics store (#1984, #1976).
 *
 * That deployment answered, and the answer will not change until someone sets
 * `CLICKHOUSE_URL`: it is a configuration rolter supports, not an outage. It
 * used to render `LoadError`, whose red `role="alert"` put it in the same voice
 * as a 500 and had a screen reader announce it as urgent on every visit. This
 * is the same information, stated calmly as a `status`: the cause, the setting
 * in monospace, and the control plane's own words under it (#962). There is
 * no retry, because no retry can help.
 *
 * `i18nKey` names the screen's own copy, which needs a `title` and a `body`
 * under it. The body carries one `<0>` element around the setting's name, and
 * says what the screen will show once the store is there.
 */
export function AnalyticsUnavailable({ error, i18nKey }: { error: unknown; i18nKey: string }) {
  const { t } = useTranslation();
  const detail = error instanceof Error ? error.message : null;
  return (
    <div
      role="status"
      className="flex max-w-[72ch] items-start gap-3 rounded-lg border border-[color:var(--border-default)] bg-[color:var(--surface-subtle)] px-4 py-3.5"
    >
      <ChartNoAxesColumn
        aria-hidden
        className="mt-0.5 h-4 w-4 flex-none text-[color:var(--status-info-text)]"
      />
      <div className="flex min-w-0 flex-col gap-2">
        <p className="text-sm font-medium text-foreground">{t(`${i18nKey}.title`)}</p>
        <p className="text-sm leading-relaxed text-muted-foreground">
          <Trans
            i18nKey={`${i18nKey}.body`}
            components={[<code key="env" className="font-mono text-xs text-foreground" />]}
          />
        </p>
        {detail && (
          <p className="break-words font-mono text-xs text-[color:var(--text-subtle)]">{detail}</p>
        )}
      </div>
    </div>
  );
}
