import * as React from "react";
import { useTranslation } from "react-i18next";
import { useBlocker } from "react-router";

import { ConfirmDialog } from "@/components/ConfirmDialog";

// leaving a screen with an unsaved draft (#2584).
//
// `useDiscardGuard` covers a sheet closing; this covers the page itself going
// away, by in-app navigation (a nav item, the back button) and by the tab
// closing or reloading. a clean draft is never asked about.
//
// `useBlocker` needs a data router, so the app mounts one in `main.tsx` and the
// story harness does the same for a story with no router of its own.
export function useLeaveGuard(dirty: boolean): React.ReactNode {
  const { t } = useTranslation();

  // a navigation to the same location (a click on the nav item already active)
  // keeps the draft, so it is not worth a prompt
  const blocker = useBlocker(
    ({ currentLocation, nextLocation }) =>
      dirty &&
      (currentLocation.pathname !== nextLocation.pathname ||
        currentLocation.search !== nextLocation.search),
  );

  React.useEffect(() => {
    if (!dirty) return;
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      // the browser shows its own wording; setting returnValue is what makes
      // older engines ask at all
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => window.removeEventListener("beforeunload", onBeforeUnload);
  }, [dirty]);

  // a draft that stopped being dirty under the prompt — a save that landed —
  // has nothing left to lose, so let the held navigation through
  React.useEffect(() => {
    if (!dirty && blocker.state === "blocked") blocker.proceed();
  }, [dirty, blocker]);

  return (
    <ConfirmDialog
      name="leave-unsaved"
      open={blocker.state === "blocked"}
      onOpenChange={(open) => {
        if (!open && blocker.state === "blocked") blocker.reset();
      }}
      title={t("common.discardChanges")}
      description={t("common.discardBody")}
      confirmLabel={t("common.discardConfirm")}
      onConfirm={() => {
        if (blocker.state === "blocked") blocker.proceed();
      }}
    />
  );
}
