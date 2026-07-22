# rolter ui

Dashboard for rolter — Vite + React + TypeScript + Tailwind + [shadcn/ui](https://ui.shadcn.com), managed with [Bun](https://bun.sh).

## Develop

```bash
bun install          # install dependencies
bun run dev          # dev server on http://localhost:3000 (proxies /api -> :4001)
bun run build        # production build into dist/ (served by rolter-control)
bun run lint         # typecheck
bun run storybook    # component workbench on http://localhost:6006
bun run build-storybook # static Storybook build into storybook-static/
bun run test-storybook  # run the interaction (play) tests headless
```

## Add shadcn components

The base config lives in `components.json`. Add components with:

```bash
bunx shadcn@latest add button card table badge dialog input
```

Components are copied into `src/components/ui`. A starter `button` and `card` are already included.

## Storybook

Storybook (v10) uses the same local Tailwind/design-token stylesheet as the
dashboard, so it has **no runtime CDN dependency** (air-gapped-safe). The Rolter
Design System is dark-only, so `.storybook/preview.ts` renders every story on the
design's dark surface — there is no light variant to toggle.

### Add a story

Add a colocated `*.stories.tsx` beside the component, export a typed `Meta`, and
include at least one representative story. Prefer a controlled wrapper for
stateful components so their behavior can be exercised:

```tsx
import type { Meta, StoryObj } from "@storybook/react";
import { Switch } from "./switch";

const meta = {
  title: "Primitives/Switch",
  component: Switch,
} satisfies Meta<typeof Switch>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Off: Story = { render: () => <Controlled /> };
```

### Add a play (interaction) test

Attach a `play` function using `storybook/test` (built into Storybook 10) to
assert behavior, not just render. Play tests run in the Canvas and are executed
headless by `bun run test-storybook`:

```tsx
import { expect, userEvent, within } from "storybook/test";

export const Toggles: Story = {
  render: () => <Controlled />,
  play: async ({ canvasElement }) => {
    const sw = within(canvasElement).getByRole("switch");
    await userEvent.click(sw);
    await expect(sw).toHaveAttribute("aria-checked", "true");
  },
};
```

Notes:
- Portalled overlays (Dialog, Sheet) render into `document.body`, so assert with
  `within(document.body)`, not the story canvas.
- `userEvent.click` refuses to click a `disabled` control; assert `toBeDisabled()`
  and pass `{ pointerEventsCheck: 0 }` if you must force the click.
- `test-storybook` needs a running Storybook — the runner points at a served
  static build (see the `storybook` CI job in `.github/workflows/quality.yml`),
  and locally `bun run test-storybook` drives whatever is on `:6006`.

Stories currently cover the core UI-kit primitives (button, input, textarea,
select, switch, badge, tag, card, stat-card, empty-state, skeleton, tabs), the
overlays (dialog, sheet), the charts (sparkline, donut, bar/line), and the
navigation shells (`NavSidebar`, `FilterPanel`), under `src/components/ui/`.

## Structure

- `src/main.tsx` — app entry (React Query + Router)
- `src/App.tsx` — layout + routes
- `src/pages/` — Models, Keys, Logs
- `src/components/ui/` — shadcn components
- `src/lib/api.ts` — typed fetch helpers for the control API
- `src/lib/utils.ts` — `cn()` class helper
