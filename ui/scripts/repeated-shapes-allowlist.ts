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
  "section|border border-[color:var(--border-subtle)] flex flex-col gap-3.5 p-4 rounded-[10px]":
    "`SettingsPanel`'s own outer markup, hand-written in five settings screens that never adopted it. converting them is #1711 and needs each call site checked for the title/description/fieldset structure the primitive imposes",
  "div|bg-card border border-[color:var(--border-default)] flex flex-col gap-3 p-4 rounded-[10px]":
    "a card with a fixed gap and padding, one step away from `Card`. which of the two it should become is a design call, tracked in #1711",
  "div|mb-0.5 text-[0.6875rem] text-[color:var(--text-subtle)] tracking-[0.05em] uppercase":
    "the overline above a stat, in four screens. wants a named primitive rather than four copies of the same tracking, tracked in #1711",
  "section|bg-[color:var(--surface-card)] border border-[color:var(--border-subtle)] rounded-[10px]":
    "a bordered panel on the surface-card ground, in three auth screens. tracked in #1711",
  "section|border border-[color:var(--border-subtle)] flex flex-col gap-2.5 p-4 rounded-[10px]":
    "the same panel as the `gap-3.5` entry above with a tighter gap; both collapse into one primitive with a prop, tracked in #1711",
  "section|border border-[color:var(--border-subtle)] flex gap-4 items-start p-4 rounded-[10px]":
    "the row-wise variant of the same panel, tracked in #1711",
  "span|bg-[color:var(--surface-subtle)] border border-[color:var(--border-subtle)] flex flex-none h-[34px] items-center justify-center rounded-lg text-[color:var(--text-secondary)] w-[34px]":
    "the 34px square icon frame beside a row title, in three screens. tracked in #1711",
};
