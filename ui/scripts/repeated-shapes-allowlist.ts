// the shapes `check:primitives` reports that have not been pulled into a
// primitive yet (#1686).
//
// a key is `tag|class class class` with the classes sorted, and the value is
// the reason a reviewer accepted one more file carrying the copy. it is
// modelled on `src/lib/i18n/literals-allowlist.ts` rather than on a recorded
// baseline JSON: the check deletes an entry the moment the duplication drops
// back to two files, so the list can only shrink, and adding to it is a review
// decision rather than a regeneration step.
//
// everything in here today came from the rule's first run over the tree, and
// every entry names #1711, which carries the extraction for each one. an entry
// whose reason is "not done yet" is a debt, not a decision — the goal is an
// empty object.
import type { ShapeAllowList } from "./check-ui-primitives";

export const REPEATED_SHAPES: ShapeAllowList = {
  "div|flex flex-col gap-3.5 max-w-[840px] mx-auto p-[22px]":
    "the deployment-settings page shell, in all eight settings screens and in each of their loading, error and content states. a `SettingsPage` wrapper is #1711's first item; it is the largest of the nine and the one worth doing on its own",
};
