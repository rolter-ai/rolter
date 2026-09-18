import { ShieldAlert } from "lucide-react";
import { useTranslation } from "react-i18next";

/**
 * Explains why a per-user screen cannot be served, instead of failing bare.
 *
 * The self-service endpoints under `/api/v1/me/` need a signed-in local
 * account. In open mode the control plane has no accounts at all, so every one
 * of them 401s while the admin routes beside them pass — which reads as "my
 * login is broken" rather than "this deployment cannot serve this screen"
 * (#942). Naming the cause and the remedy is the whole point of this block.
 */
export function SelfServiceUnavailable() {
  const { t } = useTranslation();
  return (
    <div className="flex items-start gap-3 rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--red-tint)] p-4">
      <ShieldAlert
        aria-hidden
        className="mt-0.5 h-4 w-4 flex-none text-[color:var(--status-danger-text)]"
      />
      <div className="min-w-0 space-y-1">
        <p className="text-sm font-medium text-foreground">{t("account.openMode.title")}</p>
        <p className="text-sm text-muted-foreground">{t("account.openMode.body")}</p>
      </div>
    </div>
  );
}
