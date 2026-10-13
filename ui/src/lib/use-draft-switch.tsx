import * as React from "react";

import { useDiscardGuard } from "@/components/DiscardGuard";

// changing what the workbench is editing (another template, skill or version)
// while the draft holds edits (#2115).
//
// the workbench's draft is a copy of the selected version, re-seeded whenever
// the selection changes, so a click on the index or the version rail would
// overwrite the edits without a word. this asks first, through the same
// discard prompt a sheet raises (`useDiscardGuard`): `request(change)` runs
// `change` at once while the draft is clean, and otherwise holds it behind the
// prompt, where Discard runs it and Cancel drops it with the edits untouched
export function useDraftSwitch(dirty: boolean): {
  request: (change: () => void) => void;
  prompt: React.ReactNode;
} {
  const held = React.useRef<(() => void) | undefined>(undefined);
  const { close, prompt } = useDiscardGuard({
    dirty,
    // `close` reports "dismissed" with `false`; here that is the change going ahead
    onOpenChange: (open) => {
      if (open) return;
      const change = held.current;
      held.current = undefined;
      change?.();
    },
  });
  const request = React.useCallback(
    (change: () => void) => {
      held.current = change;
      close();
    },
    [close],
  );
  return { request, prompt };
}
