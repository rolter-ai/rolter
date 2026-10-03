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
    fontSize: "0.625rem"
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
  header: "52px"
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
- **Hairlines** (`border-subtle`, `border-default`, `border-strong`): translucent white borders at
  8%, 15% and 24%. They do the separating work shadows would do elsewhere.

### Status

Each status hue is a pair. The fill (`status-success`, `status-warning`, `status-info`,
`status-danger`) carries dots, bars, borders and `/15` tints; the `-text` half carries the label.
Warning has one value because the amber already clears AA as text.

Charts read `--chart-1` … `--chart-8` and `--chart-other` by index, and avatar chips read
`--avatar-1` … `--avatar-6`; both palettes live in `ui/src/index.css` with their measured ratios.
The chart sequence is red, zinc-400, blue, green, amber, cyan, zinc-500, zinc-300; no two entries are
closer than CIE76 ΔE 17, also under the three colour-vision deficiencies.

### Named Rules

**The One Red Voice Rule.** A view has one red action at most: the Folk Red brand button, or a
single destructive action. Otherwise Folk Red marks state (the active page, a switch that is on)
and Accent Red marks a selection. If two things on a screen are red for different reasons, one of
them is wrong.

**The Shape-or-Glyph Rule.** Every hue that colours both shapes and text comes as a pair. Pick the
half by what it paints: a dot, bar or border takes the fill; a letter takes the `-text` token.

**The Token-Only Rule.** No hex, no raw Tailwind palette colour (`text-emerald-600`), no `dark:`
variant. Reference a token: `text-[color:var(--status-danger-text)]`.

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
  in Subtle Zinc. The `--text-2xs` token carries the size but is not registered with Tailwind yet,
  so call sites spell it out (#1990).
- **Badge** (500, 10px): badge text only.
- **Mono** (400, 12px): identifiers and values inline with body text; sheet subtitles.
- **Figure** (Geist Mono 500, 28px, tight tracking): the headline number of a stat card.

### Named Rules

**The Copyable-Is-Mono Rule.** If someone might paste it into a terminal or a config file, set it
in Geist Mono, verbatim.

**The No-Substitute Rule.** Never swap Geist for Inter, Roboto or Arial; fonts are vendored from
`node_modules`, never fetched from a CDN.

## Layout

The shell is a fixed 232px sidebar (a drawer below `md`) beside a content column with a 52px
header band. Screens pad their header and body with a 22px horizontal gutter; cards pad 24px;
table cells pad 16px by 8px. Spacing follows Tailwind's 4px scale.

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
- **Lifted** (`box-shadow: 0 12px 32px rgba(0, 0, 0, 0.55)`, `--shadow-lg`): meant for dialogs and
  menus. Those currently use Tailwind's stock `shadow-lg` utility, which is not this token, and the
  sign-in card rests on a `shadow-2xl`; both are tracked in #1990.
- **Sheet edge** (`box-shadow: -14px 0 44px rgba(0, 0, 0, 0.42)`): the right-hand sheet over a 50%
  black scrim, mirrored for the nav drawer below `md`.

### Named Rules

**The Borders-Not-Shadows Rule.** A resting surface separates with a hairline and a tone step.
A shadow means "this is floating over the page".

## Shapes

Corners are gently rounded and consistent: 6px for badges and small chips, 8px for buttons,
inputs and nav items, 10px for cards, 14px for large containers, full rounding for status dots,
pills and switch tracks. The only non-rectangular motif is the cross-stitch: the mark's 29 rounded
squares and the ornament's 45° checkerboard.

## Components

Compact, crisp, quiet: components are small, sharp-edged by hairline, and say nothing until there
is state to show.

### Buttons

- **Shape:** gently rounded (8px), 36px tall; `sm` is 32px with 12px text.
- **Primary:** Near White fill with Graphite text. The default action button is neutral, not red.
- **Brand:** Folk Red fill with white text, hovering to `red-accent-press`. Reserved for the one
  action a screen exists for, such as signing in, accepting an invite or turning on two-factor.
  It is hand-written at each call site today rather than a `Button` variant (#1993).
- **Outline / Ghost:** Graphite or transparent with Near White text, hovering to the Zinc Band.
- **Destructive:** a Folk Red surface; its confirm lives in `ConfirmDialog`.
- **Focus:** a 1px `focus-ring` ring on `:focus-visible`; disabled drops to 50% opacity.

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

10px medium text on a 20px pill with 6px corners. Status tones tint the status fill at 15% and set
the label in the `-text` half; the accent tone is Red Tint with Folk Red text.

### Tables

Two header shapes ship. `Table` has a Zinc Band header row in 12px medium Muted Zinc; the
`ListTable` grid used by most list screens has an 11px uppercase Overline header in Subtle Zinc.
Body cells are 14px Soft Zinc with `border-subtle` between rows and hover to the Zinc Band. Mono
columns drop to 12px Near White.

### Stat card

A 12px label over a 28px Geist Mono figure with an optional muted unit and a status-coloured
delta. The KPI tile of every analytics screen.

### Вышивка ornament (signature)

`.vyshivka-rule` is a 10px cross-stitch checkerboard in Folk Red; `.vyshivka-dots` is the quieter
dotted divider. They appear under the screen header, as a short thread in an empty state and on the
sign-in screen, and nowhere else by default.

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
