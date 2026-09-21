# Dashboard theme

## The dashboard is dark-only

There is one palette and no theme toggle. `ui/src/index.css` is that palette:
`:root` holds the shadcn HSL contract, the zinc ramp, the folkloric red accent,
the status hues, the surfaces and the type scale, and `:root` also declares
`color-scheme: dark` so the browser paints scrollbars and form controls to
match. There is no light theme to fall back to and no plan for one.

Because of that, **`dark:` variants are banned**. Tailwind's `darkMode` option
is not set, so `dark:bg-…` and `dark:text-…` compile to nothing at all — a
class that looks like a decision but ships as dead weight. Write the one value
you mean:

```tsx
// no
<p className="text-amber-600 dark:text-amber-500">…</p>
// yes
<p className="text-[color:var(--status-warning-text)]">…</p>
```

## Colour comes from the tokens

Never hard-code a hex, and never reach for a raw Tailwind palette colour
(`text-emerald-600`, `bg-blue-500/15`, `text-red-400`). Those bypass the design
system: they are not retunable, they are not contrast-checked against the
rolter surface, and they drift out of family with everything around them.
Reference the token instead — `text-[color:var(--status-danger)]`,
`bg-[color:var(--red-tint)]`, `border-[color:var(--border-subtle)]`.

## Status colours come in two flavours

Each status hue ships as a pair, and picking the wrong half is a contrast bug:

| Token                                                                                             | Use for                                               |
| ------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| `--status-success` / `--status-warning` / `--status-info` / `--status-danger`                     | fills: dots, bars, chart series, `/15` tints, borders |
| `--status-success-text` / `--status-warning-text` / `--status-info-text` / `--status-danger-text` | text: badge labels, inline warnings, delta figures    |

The fill hues are tuned to carry a shape. As _text_ they are marginal or fail
outright — `--status-info` is 4.34:1 on its own `/15` tint over
`--surface-base` and `--status-danger` is 3.93:1, both under the 4.5:1 WCAG AA
floor for body text. Each `-text` token is the same OKLCH hue and chroma with
the lightness lifted until it clears 4.5:1. `--status-warning-text` equals
`--status-warning`, because the amber already cleared; it exists so a component
author never has to know which hue happens to pass.

**Check every surface, not just the base one.** A `/15` tint gets lighter with
the surface under it, so a Badge on a card, in a sheet, or on a selected row
sits on a different background than the same Badge on the page. The tokens were
first tuned against `#111113` alone, and axe found what that leaves behind: a
`success` Badge on `--surface-subtle` was 4.23:1 (#1181). The recorded number is
now the worst of eight — each of the four surfaces, bare and tinted:

| Token                   | Value     | Worst ratio                           |
| ----------------------- | --------- | ------------------------------------- |
| `--status-success-text` | `#38c163` | 5.21:1 (tint over `--surface-subtle`) |
| `--status-warning-text` | `#f59e0b` | 5.22:1                                |
| `--status-info-text`    | `#6ba1ff` | 4.79:1                                |
| `--status-danger-text`  | `#ff6f66` | 4.78:1                                |

The `Badge` component in `ui/src/components/ui/badge.tsx` is the reference
implementation: `/15` tint off the fill hue, label off the matching `-text`
token. Its `AllTones` story asserts the computed label colour still resolves to
the token, so a tone that drifts back to a raw palette colour fails the story
tests.

When you add a status hue, add both halves, check the ratio against all four
surfaces _and_ against the tint the text will sit on, and record the worst
number in the comment beside the token.

## The same split applies outside the status hues

Three more pairs exist for the same reason, and the rule is identical: the
darker half carries a shape, the lighter half carries a glyph.

| Shape                       | Glyph                                      | Why                                                                                                                                                                             |
| --------------------------- | ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--zinc-500` (`#71717a`)    | `--text-subtle` → `--zinc-450` (`#93939e`) | `#71717a` is 3.90:1 on `--surface-base` and 3.08:1 on the `--surface-subtle` list-header band. `#93939e` is 6.20:1 / 4.90:1                                                     |
| `--red-folk` (`#b41d21`)    | `--red-folk-text` (`#ff5a3c`)              | the вышивка red is 2.82:1 on the base surface — an ornament hue. The text half is 6.09:1 on base, 4.81:1 on `--surface-subtle`                                                  |
| `--destructive` (`#b41d21`) | `--status-danger-text`                     | `--destructive` is a _surface_: `bg-destructive`, `border-destructive/30`, `bg-destructive/5`. It used to be `#e53935`, which carries `--destructive-foreground` at only 4.05:1 |

`text-destructive` is therefore gone from the dashboard: a destructive _label_
reads `text-[color:var(--status-danger-text)]`. `--danger-text` is an alias of
the same token.

## A translucent tint takes on what is behind it

`--red-tint` is `rgba(255, 64, 23, 0.12)`, so the colour a reader actually sees
is the tint composited over whatever it is laid on — and it gets lighter with
that surface, taking the contrast of the text on it down too. `--text-subtle` is
5.55:1 on the tint over `--surface-base` and 4.36:1 on the tint over
`--surface-subtle`, under the AA floor. That is how `LoadError`'s detail line
failed axe once a screen put the alert inside a `--surface-subtle` band (#1725).

A component that can land inside a panel as well as on the page paints
`--red-tint-opaque` instead: the same tint composited onto `--surface-base` with
`color-mix()`, so it is opaque and its text reads the `--surface-base` numbers
wherever it sits. `LoadError` is the one that needs it, since it carries
`--text-subtle`; its `OnSubtleSurface` story renders it inside the lighter panels
and axe fails that story if the background turns translucent again. Keep
`--red-tint` for hovers, chips and badges, whose text is chosen to clear AA on
the tint over every surface.

## Categorical palettes are tokens too

A series colour and an avatar chip are picked by _index_, not by meaning, so
they were written out as arrays inside the components that needed them — five
of them, one in raw hex. That array is the same failure as a hard-coded hex
everywhere else: not retunable, checked against nothing, and free to drift. The
next entry somebody adds has nothing stopping it from being another `#b8860b`,
which is exactly how the gold avatar chip came to carry white initials at
3.25:1 (#1181, #1245).

Both palettes live in `ui/src/index.css`:

| Tokens                                     | Used by                                                                            | Floor                                               |
| ------------------------------------------ | ---------------------------------------------------------------------------------- | --------------------------------------------------- |
| `--chart-1` … `--chart-8`, `--chart-other` | `donut.tsx`, `scatter-plot.tsx`, `line-chart.tsx`, `Dashboard.tsx`'s provider bars | 3:1 — a fill carries a shape                        |
| `--avatar-1` … `--avatar-6`                | the `Users.tsx` chips                                                              | 4.5:1 against `#ffffff` — the chip carries initials |

The ratio for each entry is recorded in the comment beside it. For the chart
hues that is the worst of the four surfaces — a chart can sit on a card, a
sheet or a panel, and the donut draws its slices over a `--surface-subtle`
track — which is `--surface-subtle` (`#27272a`) for every entry:

| Token           | Value                        | Worst ratio (`#27272a`) | On `#111113` |
| --------------- | ---------------------------- | ----------------------- | ------------ |
| `--chart-1`     | `--red-600` `#e5342a`        | 3.44:1                  | 4.36:1       |
| `--chart-2`     | `--zinc-400` `#a1a1aa`       | 5.81:1                  | 7.36:1       |
| `--chart-3`     | `--status-info` `#3b82f6`    | 4.05:1                  | 5.13:1       |
| `--chart-4`     | `--status-success` `#16a34a` | 4.52:1                  | 5.72:1       |
| `--chart-5`     | `--status-warning` `#f59e0b` | 6.94:1                  | 8.78:1       |
| `--chart-6`     | `--red-500` `#ff4017`        | 4.25:1                  | 5.39:1       |
| `--chart-7`     | `--zinc-500` `#71717a`       | 3.08:1                  | 3.90:1       |
| `--chart-8`     | `--zinc-300` `#d4d4d8`       | 10.08:1                 | 12.76:1      |
| `--chart-other` | `--zinc-700` `#3f3f46`       | 1.43:1, exempt          | 1.81:1       |

`--chart-1` used to be `--red-folk` (2.23:1 worst) and `--chart-7` used to be
`--zinc-600` (1.93:1 worst). Both were lifted in #1269. `--chart-1` matters
most: as the first colour it paints the largest donut slice and the top
provider bar. axe cannot catch this, because its `color-contrast` rule only
checks text, so the story gate stays green whatever the chart hues are.

`--chart-other` is exempt on purpose. It is the tail a donut rolls its long
series into, it is meant to be quieter than the named slices, and it never
carries meaning alone: its legend row names it and gives its share as text at
full contrast. Lifting it past 3:1 would put it between `--chart-7` and
`--chart-2`, where it would read as one more named series. Never use it for a
series that has no legend row. Any other new entry has to clear 3:1 on all four
surfaces.

A component reads the sequence by index and never re-lists the hues:

```tsx
const PALETTE = ["var(--chart-1)", "var(--chart-2)", …];
```

Single-series defaults — a sparkline's stroke, a bar chart's fill — are a
different thing and stay on the semantic token they already use. The
categorical tokens are for "the nth of several", where the only thing the
colour means is "not the previous one".

## Never dim a live region with `opacity`

Container opacity fades the glyphs toward the page background while the
background itself does not move, so a 0.55 wrapper takes `--text-muted` from
7.36:1 to about 3.0:1 and tells assistive technology nothing at all. Two
replacements cover every case the dashboard had:

- a **form group gated behind a toggle** becomes `<fieldset disabled>`. The
  controls inside already carried `disabled`, so this is simply the truth; axe
  skips contrast inside a disabled fieldset, and the browser stops the fields
  from taking input for a feature that is off.
- an **inactive row or card** — a disabled key, a blocked account, a retired
  business unit — gets a quieter band (`bg-[color:var(--surface-subtle)]/60`)
  instead. The state is already spelled out by the switch or badge in the row;
  the fade was only ever decoration, and it cost the row its legibility.

## Every story is an accessibility test

`ui/.storybook/test-runner.ts` runs axe over the whole document after each
story's play function, and fails the story on **any violation at any impact**
(`wcag2a`, `wcag2aa`, `best-practice`; the disabled rules are named with their
reasons in `DISABLED_RULES` — Storybook's own iframe, plus the three page-level
landmark rules a single-component story cannot satisfy, which the two stories
that mount a whole page switch back on, see
[Testing](testing.md#every-story-is-also-an-axe-test)). Adding a
story therefore adds a contrast and a semantics check for whatever it renders,
in every state it renders — empty, loading and error included.

A failure looks like this:

```
● Screens/Keys › Loaded › smoke-test
  1 accessibility violation was detected
```

with the detail printed above it as two tables: the first names the rule
(`color-contrast`, `button-name`, `label`, …) and the second gives the CSS
selector and the offending HTML. Reproduce a single story with

```bash
cd ui && bun run build-storybook
python3 -m http.server 6199 --directory storybook-static &
bun run test-storybook --url http://127.0.0.1:6199 -- -t "Keys"
```

Read the rule, not the pixel count: `color-contrast` names the exact foreground,
background and ratio it measured, and the fix is nearly always a token swap from
this page rather than a new colour. A story may opt out with
`parameters: { a11y: { disable: true } }`, but it has to say why beside it — no
story in the dashboard currently needs to.

## The identity around the tokens

This page governs the dashboard's _interior_: which token a component reaches
for and why. The identity those tokens express — the cross-stitch mark, its
clear space and minimum size, when `--red-500` is the accent and `--red-folk`
is ornament, where the вышивка rule belongs and where it does not, and how the
wordmark is set — is documented once for everybody, in
[the brand usage guidelines](../../user-docs/community/brand.mdx) on the
end-user docs site.

Read it before you place the mark anywhere, add a `.vyshivka-rule` to a screen,
or produce an asset that carries the brand (a slide, a README header, the social
preview at `assets/og.svg`). The rules there are the reason the constraints here
exist.
