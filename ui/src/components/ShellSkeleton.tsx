import { useTranslation } from "react-i18next";

import { Skeleton } from "@/components/ui/skeleton";

// the shell while the stored session token is being revalidated against
// /api/v1/auth/me (#1196). the boot check takes one request, and the two
// honest things to show for it are the shape the dashboard is about to have —
// rail on the left, header and content on the right — or the login screen.
// showing login would be a lie every time the token turns out to be fine, so
// this is what stands in.
export function ShellSkeleton() {
  const { t } = useTranslation();
  return (
    <div
      role="status"
      aria-busy="true"
      aria-label={t("shell.checkingSession")}
      className="flex h-screen bg-[color:var(--surface-app)]"
    >
      <div className="hidden w-[52px] flex-col gap-3 border-r border-[color:var(--border-subtle)] px-2 py-3 md:flex lg:w-[var(--sidebar-width)]">
        <Skeleton height={28} radius={8} />
        {/* the scope switcher, under the brand */}
        <Skeleton height={34} radius={8} />
        <Skeleton height={30} radius={8} />
        <div className="flex flex-col gap-1.5 pt-2">
          {Array.from({ length: 8 }, (_, i) => (
            <Skeleton key={i} height={26} radius={6} />
          ))}
        </div>
        <div className="mt-auto">
          <Skeleton height={40} radius={8} />
        </div>
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-4 border-l border-[color:var(--border-subtle)] bg-background p-6">
        <Skeleton width={220} height={22} radius={6} />
        <Skeleton width={340} height={14} radius={6} />
        <div className="grid grid-cols-1 gap-3 pt-2 sm:grid-cols-2 lg:grid-cols-4">
          {Array.from({ length: 4 }, (_, i) => (
            <Skeleton key={i} height={96} radius={10} />
          ))}
        </div>
        <Skeleton height={220} radius={10} />
      </div>
    </div>
  );
}
