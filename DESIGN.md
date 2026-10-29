---
name: rolter
description: Dark-only control plane for a self-hosted AI gateway, graphite with one folk-red thread
colors:
  surface-app: "#18181b"
  surface-base: "#111113"
  surface-elevated: "#1f1f23"
  surface-subtle: "#27272a"
  text-primary: "#fafafa"
  text-secondary: "#d4d4d8"
  text-muted: "#a1a1aa"
  text-subtle: "#93939e"
  zinc-shape: "#71717a"
  border-subtle: "rgba(255, 255, 255, 0.08)"
  border-default: "rgba(255, 255, 255, 0.15)"
  border-strong: "rgba(255, 255, 255, 0.24)"
  focus-ring: "#8c8c8c"
  red-accent: "#ff4017"
  red-accent-hover: "#e5342a"
  red-accent-press: "#c22118"
  red-folk: "#b41d21"
  red-folk-text: "#ff5a3c"
  red-tint: "rgba(255, 64, 23, 0.12)"
  status-success: "#16a34a"
  status-success-text: "#38c163"
  status-warning: "#f59e0b"
  status-info: "#3b82f6"
  status-info-text: "#6ba1ff"
  status-danger: "#e53935"
  status-danger-text: "#ff6f66"
typography:
  headline:
    fontFamily: "Geist Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "1.125rem"
    fontWeight: 600
    lineHeight: 1.2
    letterSpacing: "-0.025em"
  title:
    fontFamily: "Geist Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 500
    lineHeight: 1.43
  body:
    fontFamily: "Geist Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 400
    lineHeight: 1.43
  body-compact:
    fontFamily: "Geist Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.8125rem"
    fontWeight: 400
    lineHeight: 1.5
  label:
    fontFamily: "Geist Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.75rem"
    fontWeight: 500
    lineHeight: 1.33
  badge:
    fontFamily: "Geist Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.6875rem"
    fontWeight: 500
    lineHeight: 1
  overline:
    fontFamily: "Geist Variable, ui-sans-serif, system-ui, sans-serif"
    fontSize: "0.6875rem"
    fontWeight: 400
    lineHeight: 1.33
    letterSpacing: "0.07em"
  mono:
    fontFamily: "Geist Mono Variable, ui-monospace, SFMono-Regular, monospace"
    fontSize: "0.75rem"
    fontWeight: 400
    lineHeight: 1.33
  figure:
    fontFamily: "Geist Mono Variable, ui-monospace, SFMono-Regular, monospace"
    fontSize: "1.75rem"
    fontWeight: 500
    lineHeight: 1
    letterSpacing: "-0.025em"
rounded:
  sm: "6px"
  md: "8px"
  lg: "10px"
  xl: "14px"
  full: "9999px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "16px"
  gutter: "22px"
  lg: "24px"
  sidebar: "232px"
  sheet: "580px"
components:
  button-primary:
    backgroundColor: "{colors.text-primary}"
    textColor: "{colors.surface-base}"
    typography: "{typography.title}"
    rounded: "{rounded.md}"
    height: "36px"
    padding: "8px 16px"
  button-primary-hover:
    backgroundColor: "rgba(250, 250, 250, 0.9)"
    textColor: "{colors.surface-base}"
  button-outline:
    backgroundColor: "{colors.surface-base}"
    textColor: "{colors.text-primary}"
    typography: "{typography.title}"
    rounded: "{rounded.md}"
    height: "36px"
    padding: "8px 16px"
  button-outline-hover:
    backgroundColor: "{colors.surface-subtle}"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.md}"
    height: "36px"
  button-brand:
    backgroundColor: "{colors.red-folk}"
    textColor: "#ffffff"
    typography: "{typography.title}"
    rounded: "{rounded.md}"
    height: "36px"
    padding: "8px 16px"
  button-brand-hover:
    backgroundColor: "{colors.red-accent-press}"
  button-destructive:
    backgroundColor: "{colors.red-folk}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.md}"
    height: "36px"
    padding: "8px 16px"
  input:
    backgroundColor: "{colors.surface-subtle}"
    textColor: "{colors.text-primary}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    height: "36px"
    padding: "4px 12px"
  card:
    backgroundColor: "{colors.surface-base}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.lg}"
    padding: "24px"
  badge-neutral:
    backgroundColor: "{colors.surface-subtle}"
    textColor: "{colors.text-muted}"
    typography: "{typography.badge}"
    rounded: "{rounded.sm}"
    height: "20px"
    padding: "0 8px"
  badge-success:
    backgroundColor: "rgba(22, 163, 74, 0.15)"
    textColor: "{colors.status-success-text}"
    typography: "{typography.badge}"
    rounded: "{rounded.sm}"
    height: "20px"
    padding: "0 8px"
  badge-danger:
    backgroundColor: "rgba(229, 57, 53, 0.15)"
    textColor: "{colors.status-danger-text}"
    typography: "{typography.badge}"
    rounded: "{rounded.sm}"
    height: "20px"
    padding: "0 8px"
  badge-accent:
    backgroundColor: "{colors.red-tint}"
    textColor: "{colors.red-folk-text}"
    typography: "{typography.badge}"
    rounded: "{rounded.sm}"
    height: "20px"
    padding: "0 8px"
  nav-item:
    backgroundColor: "transparent"
    textColor: "{colors.text-muted}"
    typography: "{typography.body}"
    rounded: "{rounded.md}"
    padding: "6px 8px"
  nav-item-active:
    backgroundColor: "{colors.surface-subtle}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.md}"
    padding: "6px 8px"
  table-header:
    backgroundColor: "{colors.surface-subtle}"
    textColor: "{colors.text-muted}"
    typography: "{typography.label}"
    padding: "8px 16px"
  list-header:
    backgroundColor: "{colors.surface-subtle}"
    textColor: "{colors.text-subtle}"
    typography: "{typography.overline}"
    padding: "9px 16px"
  table-cell:
    textColor: "{colors.text-secondary}"
    typography: "{typography.body}"
    padding: "8px 16px"
  switch-on:
    backgroundColor: "{colors.red-folk}"
    rounded: "{rounded.full}"
  chip-selected:
    backgroundColor: "{colors.red-tint}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.md}"
---

# Design System: rolter

<!-- Derived from ui/src/index.css, ui/tailwind.config.js and ui/src/components/ui/. When this file
and the code disagree, the code wins: fix the drift, then refresh this file with
`/impeccable document`. Contrast numbers and the reasoning behind each token live in
docs/dev-docs/development/dashboard-theme.md; the mark and wordmark rules live in
docs/user-docs/community/brand.mdx. -->

## Overview

**Creative North Star: "The Stitched Console"**

rolter is a graphite instrument panel with one red thread through it. The panel is for people
running a gateway: it is dense, dark and quiet, and it lets numbers, health and cost carry the
screen. The thread comes from Slavic folk embroidery, the shared cross-stitch vocabulary that runs
from the Urals to the Carpathians rather than the pattern of any one country. It shows up as the
logo, the active-nav mark, a switch that is on, and the one primary action a view is built around.
Everything else is zinc.

Density is deliberate. The document default is 13px, controls and tables are 14px, and a screen
fits a table, its filters and a header without scrolling on a laptop. Surfaces separate by tone
and hairline borders, not by shadows. Colour appears only to say something: a status, a selection,
the brand. The dashboard is dark-only; there is no light theme to design for.

The folk layer is restrained on purpose. A screen gets at most one or two stitched ornaments, and
the ornament never sits under text or competes with data.

**Key Characteristics:**

- Dark-only zinc ramp, near-black `surface-base` on a slightly lighter `surface-app` backdrop.
- One red voice per view; the rest is neutral.
- Compact, crisp, quiet components with translucent hairline borders.
- Geist for interface text, Geist Mono for every identifier, number and money value.
- Contrast is enforced: text clears 4.5:1 and shapes 3:1 on every surface they can land on.

## Colors

A single neutral zinc ramp carries almost every pixel; two reds and four status hues add meaning.

### Primary

- **Accent Red** (`red-accent`): the bright stitch of the mark. In the interface it marks a
  _selection_: the border of a selected chip or a locked field, and a chart series. It is not a
  button fill. `red-accent-hover` is the first chart series; `red-accent-press` is the brand
  button's hover.
- **Folk Red** (`red-folk`): the deep embroidery red from the logo's inner ring. It is a _shape_
  colour: the brand button fill, the active-nav tick, a switch track that is on, the вышивка rule
  and destructive surfaces. It is 2.82:1 on `surface-base`, so it never colours text.
- **Folk Red, text half** (`red-folk-text`): the same family lifted to 6.09:1 for eyebrows,
  accent badges and code keywords.
- **Red Tint** (`red-tint`): a 12% wash of Accent Red for selected chips, accent badges and hover
  on red-bordered controls. Where text must read inside a lighter panel, use the opaque
  `--red-tint-opaque` from the stylesheet instead.

### Neutral

- **App Backdrop** (`surface-app`): behind the content panel and the sidebar.
- **Graphite** (`surface-base`): the page, cards and table bodies.
- **Raised Graphite** (`surface-elevated`): popovers, menus, sheets.
- **Zinc Band** (`surface-subtle`): table header bands, inputs, hovered rows, the active nav item.
- **Near White** (`text-primary`): headings, values, primary button fill.
- **Soft Zinc** (`text-secondary`): table cell text.
- **Muted Zinc** (`text-muted`): descriptions, idle nav items, table headers.
- **Subtle Zinc** (`text-subtle`): the dimmest text allowed; 4.90:1 on the Zinc Band.
- **Zinc Shape** (`zinc-shape`): hairlines, icons at rest, chart fills. Never text.
- **Selection** (`selection-bg`, `selection-text`): selected text is Near White on the brand red (`red-folk`), 6.41:1 and opaque, so it reads the same on Graphite, Raised Graphite and the Zinc Band. The fill is 2.82:1 / 2.45:1 / 2.23:1 against those surfaces; the glyph ratio is what carries it. One `::selection` rule in `index.css`.
- **Hairlines** (`border-subtle`, `border-default`, `border-strong`): translucent white borders at
  8%, 15% and 24%. They do the separating work shadows would do elsewhere.

### Status

Each status hue is a pair. The fill (`status-success`, `status-warning`, `status-info`,
`status-danger`) carries dots, bars, borders and `/15` tints; the `-text` half carries the label.
Warning has one value because the amber already clears AA as text.

Charts read `--chart-1` … `--chart-8` by index, and avatar chips read
`--avatar-1` … `--avatar-6`; both palettes live in `ui/src/index.css` with their measured ratios.
The chart sequence is red, zinc-400, blue, green, amber, cyan, zinc-500, zinc-300; no two entries are
closer than CIE76 ΔE 17, also under the three colour-vision deficiencies.

### Named Rules

**The One Red Voice Rule.** A view has one red action at most: the Folk Red brand button, or a
single destructive action. Otherwise Folk Red marks state (the active page, a switch that is on)
and Accent Red marks a selection. If two things on a screen are red for different reasons, one of
them is wrong. A state that a screen draws hundreds of times is therefore never red: the allowed
cells of the permission matrix are neutral (see Permission matrix).

**The Shape-or-Glyph Rule.** Every hue that colours both shapes and text comes as a pair. Pick the
half by what it paints: a dot, bar or border takes the fill; a letter takes the `-text` token.

**The Token-Only Rule.** No hex, no raw Tailwind palette colour (`text-emerald-600`), no `dark:`
variant. Reference a token: `text-[color:var(--status-danger-text)]`.

**The One-Source Rule.** A value has one token that carries it. The shadcn names Tailwind reads
(`bg-background`, `bg-card`, `bg-popover`, `bg-secondary`, `bg-muted`, `bg-accent`,
`text-muted-foreground`, `border-input`, `ring-ring`) are aliases of the design tokens in
`ui/src/index.css` (`--surface-base`, `--surface-card`, `--surface-elevated`, `--surface-subtle`,
`--surface-hover`, `--text-muted`, `--zinc-800`), not second copies of their colours, so retuning
`--zinc-800` moves `bg-muted` with `--surface-subtle` (#1993). Either spelling is fine at a call
site; a new value goes in the design tokens first. The default `border` colour stays the opaque
`--zinc-800`, which is what a bare `border` has always painted; the translucent hairlines are the
three `--border-*` tokens. This is a decision, not a gap (#3283): a bare `border` is opaque while
the 200-odd `--border-subtle` uses are translucent, and making the default translucent would move
every bare border a few levels (a little darker on the base surface, lighter on elevated ones), so it
stays opaque until someone looks at each screen with that change in hand. New borders use the
`--border-*` tokens; the bare `border` is for the shadcn contract's own components.

## Typography

**Display Font:** none; the dashboard has no display tier.
**Body Font:** Geist Variable (with `ui-sans-serif`, `system-ui`)
**Label/Mono Font:** Geist Mono Variable (with `ui-monospace`, `SFMono-Regular`)

**Character:** Geist is a neutral grotesk with full Cyrillic coverage, so `ru` copy sets as cleanly
as `en`. Geist Mono carries everything a person might copy: model names, provider slugs, key
prefixes, config keys, token counts, latency and money.

### Hierarchy

- **Headline** (600, 18px, tight tracking): the screen title in `ScreenHeader`, card titles and
  sheet titles. One per region.
- **Title** (500, 14px): buttons, tabs, empty-state titles.
- **Body** (400, 14px): tables, inputs, nav items, descriptions.
- **Body compact** (400, 13px): the document default for everything that does not set its own size.
- **Label** (500, 12px): `Table` headers, stat-card labels, helper lines.
- **Overline** (400, 11px, uppercase, 0.07em tracking): `ListTable` headers and small eyebrows,
  in Subtle Zinc. `text-2xs` (`--text-2xs`, registered in `@theme`).
- **Badge** (500, 11px): badge text, and the floor for any text. It is the same `text-2xs` step as
  the overline, in medium weight and without the uppercase tracking.
- **Mono** (400, 12px): identifiers and values inline with body text; sheet subtitles.
- **Figure** (Geist Mono 500, 28px, tight tracking): the headline number of a stat card.
  `text-figure` (`--text-figure`).

A size not on this list is not spelled out as `text-[11px]` or `text-[0.625rem]`: it either is one
of these steps, or it is a new step registered in the `@theme` block of `ui/src/index.css` (#1990).

**The 11px floor.** HTML text is never set below 11px. The badge was 10px until the floor was
raised (#2230): screens put primary data in badges (a price on Pricing, a cap on Limits), and 10px
text was below the detector's `undersized-ui-text` line on most of them. The alternative, keeping
10px for status words and moving data out of badges, was rejected: a badge stays free to carry a
number, so it must stay legible as one. The old `--text-3xs` step is gone with it; a label or
glyph that was set in it uses `text-2xs`. Labels inside a chart's SVG are held to the same floor at
the size the chart is drawn: `LineChart` measures its frame and sets its labels in pixels, so they
are 11px at every width. `BarChart` (vertical) and `ScatterPlot` share its measuring hook
(`useMeasuredWidth`), so no chart is an exception (#3450, #3457). One thing sits outside the scale: the `aria-hidden` provider monogram,
which is a mark rather than text.

### Named Rules

**The Copyable-Is-Mono Rule.** If someone might paste it into a terminal or a config file, set it
in Geist Mono, verbatim.

**The Wordmark Rule.** The wordmark is the word **rolter** as
`docs/user-docs/community/brand.mdx` sets it: Geist SemiBold, lowercase, `-0.04em` tracking, nothing
after it. The sidebar and the sign-in card follow it; the Geist Mono "rolter." the design prototype
used is retired (#1992).

**The No-Substitute Rule.** Never swap Geist for Inter, Roboto or Arial; fonts are vendored from
`node_modules`, never fetched from a CDN.

## Layout

The shell is a fixed 232px sidebar (a drawer below `md`) beside a content column. The screen
header has no height of its own: `ScreenHeader` pads 16px above and below its title and subtitle and
sizes to them, so there is no header token (#3283). Screens pad their header and body with a 22px horizontal gutter; cards pad 24px;
table cells pad 16px by 8px. Spacing follows Tailwind's 4px scale.

Settings screens use `SettingsBody`, `PageBody`'s left-aligned gutter capped at 840px, so a form
keeps a readable measure without centring: content stays at the same left edge when moving between
a list screen and a settings one (#3053).

Sheets slide in from the right at up to 580px wide and take the full width on small screens. The
page never scrolls sideways: grid and flex children carry `min-w-0`, and wide tables scroll inside
their card.

Below `sm` a sheet's footer stacks (`SheetActions`): Cancel and the primary action share the bottom
line, the primary last and widest, and anything else sits above them. A dialog never opens taller
than the window. A form dialog's fields scroll in a `DialogBody` between a header and a footer that
stay put, and any other dialog is scrolled whole by its overlay, top first.

Below the screen header sits a faint вышивка rule (10px, 28% opacity) that separates the header
from the content.

## Elevation & Depth

The system is flat. Depth comes from tone (`surface-app` → `surface-base` → `surface-elevated`)
and hairline borders. Cards have no shadow. Shadows exist only on things that float over the page.

### Shadow Vocabulary

- **Overlay** (`box-shadow: 0 4px 12px rgba(0, 0, 0, 0.45)`, `--shadow-md`): the combobox list
  and chart tooltips.
- **Lifted** (`box-shadow: 0 12px 32px rgba(0, 0, 0, 0.55)`, `--shadow-lg`): dialogs and menus.
- **Sheet edge** (`box-shadow: -14px 0 44px rgba(0, 0, 0, 0.42)`, `--shadow-sheet`): the right-hand
  sheet over a 50% black scrim, mirrored as `--shadow-drawer` for the nav drawer and the Logs filter
  rail below `md`.

The four are registered in the `@theme` block of `ui/src/index.css`, so `shadow-md`, `shadow-lg`,
`shadow-sheet` and `shadow-drawer` are these tokens and not Tailwind's stock shadows of the same
name. The sign-in card rests on the page and carries no shadow (#1990).

### Named Rules

**The Borders-Not-Shadows Rule.** A resting surface separates with a hairline and a tone step.
A shadow means "this is floating over the page".

## Shapes

Corners are gently rounded and consistent: 6px for badges and small chips, 8px for buttons,
inputs and nav items, 10px for cards, 14px for large containers, full rounding for status dots,
pills and switch tracks. They are the Tailwind scale (`rounded-sm`, `rounded-md`, `rounded-lg`,
`rounded-xl`, `rounded-full`), which derives from `--radius`; `rounded-[10px]` and `rounded-[6px]`
restate it and are not written. The only non-rectangular motif is the cross-stitch: the mark's 29 rounded
squares and the ornament's 45° checkerboard.

## Components

Compact, crisp, quiet: components are small, sharp-edged by hairline, and say nothing until there
is state to show.

### Buttons

- **Shape:** gently rounded (8px), 36px tall; `sm` is 32px with 12px text. Under
  `@media (pointer: coarse)` every size reaches a 44px floor (`pointer-coarse:h-11`, and `w-11` on
  `icon`); desktop density does not change (#2040).
- **Primary:** Near White fill with Graphite text. The default action button is neutral, not red.
- **Brand:** Folk Red fill with white text, hovering to `red-accent-press`. Reserved for the one
  action a screen exists for, such as signing in, accepting an invite or turning on two-factor.
  It is `variant="brand"` on `Button` (#1993). The sign-in, invitation, reset-password and
  two-factor screens still spell the same classes out and move over when those screens next change.
- **Outline / Ghost:** Graphite or transparent with Near White text, hovering to the Zinc Band.
- **Destructive:** a Folk Red surface; its confirm lives in `ConfirmDialog`.
- **Focus:** a 1px `focus-ring` ring on `:focus-visible`; disabled drops to 50% opacity. Under
  `forced-colors: active` box-shadows are stripped, so `index.css` puts a 2px `CanvasText` outline
  on every `:focus-visible` element (#1999).

### Chips

- **Style:** hairline border and transparent fill at rest.
- **Selected:** Accent Red border over Red Tint, Near White text (`ChipGroup`, Models filters).

### Cards / Containers

- **Corner Style:** 10px.
- **Background:** Graphite.
- **Shadow Strategy:** none; see Elevation.
- **Border:** `border-default` hairline.
- **Internal Padding:** 24px.

### Inputs / Fields

- **Style:** Zinc Band fill, hairline border, 8px corners, 36px tall.
- **Focus:** a 2px `focus-ring` ring.
- **Error / Disabled:** `FieldError` below the control, wired through `aria-describedby`; disabled
  is 50% opacity with a not-allowed cursor. A group gated behind a toggle is a disabled
  `<fieldset>`, never an opacity wrapper.
- **Dropdowns:** always `Combobox`, never a native `<select>`.

### Navigation

- **Sidebar items:** 14px, Muted Zinc at rest, hovering to Near White on the Zinc Band.
- **Active:** Zinc Band fill, Near White text and a 3px Folk Red tick on the left edge.
- **Tabs:** 14px medium; the active tab is Near White with a 2px Near White underline.

### Badges

11px medium text on a 20px pill with 6px corners. Status tones tint the status fill at 15% and set
the label in the `-text` half; the accent tone is Red Tint with Folk Red text.

The mono uppercase `Pill` (modality, origin and strategy tags) takes the same tones through its
`tone` prop, so a status has one tint whichever shape carries it, and an HTTP status is always
`HttpStatusBadge`: 2xx and 3xx success, 4xx warning, 5xx and no answer danger. A colour literal
(`rgba(…)`) in a component fails `ui/scripts/status-tints.test.ts` (#1989).

### Status pill

The gateway pill in every screen header says what the gateway's own `/readyz` answered, in one of
five states: checking, healthy, degraded, down and unknown. It is a full-round hairline pill with a
7px dot and a 12px label (`ui/src/components/ScreenHeader.tsx`, the states in
`ui/src/lib/gateway-health.ts`).

- **The dot** takes the status fill (`--status-*`); checking and unknown use `--text-subtle`, the
  same as the Logs screen's paused dot. It breathes (`.rl-pulse`) only while the latest check came
  back healthy.
- **The label** takes the `-text` half only for degraded and down. Healthy keeps a muted label: it
  is the resting state on every screen, the green dot already says it, and a green line of text on
  every header would spend colour on the state that needs no second look.
- **The face** is Geist, not Geist Mono. "gateway healthy" is a status, not something anyone pastes,
  so the Copyable-Is-Mono Rule does not reach it, the same call as the Logs streaming indicator
  (#2015).

### Tables

Two header shapes ship. `Table` has a Zinc Band header row in 12px medium Muted Zinc; the
`ListTable` grid used by most list screens has an 11px uppercase Overline header in Subtle Zinc.
Body cells are 14px Soft Zinc with `border-subtle` between rows and hover to the Zinc Band. Mono
columns drop to 12px Near White.

### Permission matrix

Roles & Permissions draws one small chip per (role, resource, action): about 53 resources by every
role makes hundreds of them, so the common state, allowed, is the quietest one that still reads.
Allowed is the action letter in Near White on the Zinc Band with a Zinc Shape border: 14.27:1 for
the letter, and 3.90:1 on Graphite and 3.60:1 on the scope band for the border (#2250). It is
neutral on purpose. Red is the colour of danger and of a view's one red action, and an allowed
cell is neither, so the state is carried by the glyphs and the legend, never by hue:

- **Allowed:** the letter, solid border.
- **Granted by a custom role:** the letter in Info Blue on a blue wash, dashed border, so it stays
  distinct from allowed in a greyscale render.
- **Superadmin only:** a lock, in Warning amber.
- **Not allowed:** a cross, in Subtle Zinc with a hairline border and no fill.
- **No such action:** a dash, with no fill and no border.

Every chip's value is also in text for a screen reader, so no state is told by colour alone.

### Stat card

A 12px label over a 28px Geist Mono figure with an optional muted unit and a status-coloured
delta. The KPI tile of every analytics screen.

### Вышивка ornament (signature)

`.vyshivka-rule` is a 10px cross-stitch checkerboard in Folk Red; `.vyshivka-dots` is the quieter
dotted divider. They appear under the screen header, as a short thread under a screen's own empty
state and on the sign-in screen, and nowhere else. `EmptyState` draws no thread unless it passes
`thread`, which only a screen-level placeholder does: never one inside a sheet, a table, a form or
the command palette (#1991). A chart with a single series draws it in neutral zinc (`--chart-2`),
not the red first series colour, so a spend line does not read as an alarm.

## Do's and Don'ts

### Do:

- **Do** take every colour from a token in `ui/src/index.css`, and add a pair when a new hue must
  carry both a shape and text.
- **Do** keep one red action per view, and colour state (active, on, selected) with Folk Red.
- **Do** set identifiers, numbers and money in Geist Mono.
- **Do** separate surfaces with a tone step and a hairline, and reserve shadows for overlays.
- **Do** check new text against all four surfaces and any tint it sits on; record the worst ratio
  beside the token.
- **Do** draw the folk layer from shared Slavic cross-stitch forms, used once or twice per screen.

### Don't:

- **Don't** colour text with `red-folk`, `zinc-shape` or a status fill hue; use the `-text` half.
- **Don't** write `dark:` variants, raw hex values or raw Tailwind palette colours.
- **Don't** dim a region with `opacity` to show it is inactive; use a disabled fieldset or a
  quieter band.
- **Don't** add a light theme, a theme toggle or a second accent colour.
- **Don't** put an ornament behind text or data, or use more than two on a screen.
- **Don't** use national flags, state colours, heraldry or any motif tied to one country.
- **Don't** use emoji as icons; icons are Lucide outlines at 16px in `currentColor`.
- **Don't** fetch fonts, icons or images from a CDN.
