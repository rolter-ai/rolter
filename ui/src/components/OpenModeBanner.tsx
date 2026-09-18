import { ShieldAlert } from "lucide-react";
import { useTranslation } from "react-i18next";

/**
 * Persistent warning shown while the control plane is serving with no admin
 * token, so every request is treated as superadmin (#970).
 *
 * The dashboard looks identical whether the control plane is wide open or
 * properly gated, and an operator has no reason to check. This is the one
 * signal that says so, which is why it is not dismissible: it goes away by
 * setting `ROLTER_ADMIN_TOKEN`, not by clicking.
 */
export function OpenModeBanner({ open }: { open: boolean }) {
  const { t } = useTranslation();
  if (!open) return null;
  return (
    <div
      role="status"
      className="flex flex-none items-start gap-2.5 border-b border-[color:var(--red-tint-strong)] bg-[color:var(--red-tint)] px-[22px] py-2.5 text-xs"
    >
      <ShieldAlert
        aria-hidden
        className="mt-px h-4 w-4 flex-none text-[color:var(--status-danger-text)]"
      />
      <p className="min-w-0 text-foreground">
        <span className="font-semibold">{t("shell.openMode.title")}</span>{" "}
        <span className="text-muted-foreground">{t("shell.openMode.body")}</span>
      </p>
    </div>
  );
}
