# rolter ui

Dashboard for rolter — Vite + React + TypeScript + Tailwind + [shadcn/ui](https://ui.shadcn.com), managed with [Bun](https://bun.sh).

## Develop

```bash
bun install          # install dependencies
bun run dev          # dev server on http://localhost:3000 (proxies /api -> :4001)
bun run build        # production build into dist/ (served by rolter-control)
bun run lint         # typecheck src, scripts, .storybook and e2e
bun run storybook    # component workbench on http://localhost:6006
bun run build-storybook # static Storybook build into storybook-static/
bun run test-storybook  # run the interaction (play) tests headless
bun run e2e          # Playwright browser e2e against a running rolter stack
```

## Browser e2e (Playwright)

`e2e/` drives the built dashboard through a real browser against a running
fake-vLLM rolter stack (the `integration/e2e` docker compose), covering the
critical journeys: login, provider CRUD, virtual-key lifecycle, and a
mount-without-errors smoke over the built screens.

Bring the stack up first (from the repo root), then run the suite:

```bash
ROLTER_KEK=$(printf 'rolter-e2e-test-kek-not-secret!!' | base64) \
  docker compose -f integration/e2e/docker-compose.e2e.yml up -d --wait
cd ui && bun run e2e        # or: bun run e2e:ui for the Playwright UI
```

How it works:

- `playwright.config.ts` starts the vite dev server as its `webServer`, which
  proxies `/api` → control:4001 and `/gw` → gateway:4000 (the ports the compose
  stack publishes).
- `e2e/global-setup.ts` seeds a fresh tenant + admin user via the control admin
  API (`e2e/seed.ts`) and writes an authenticated `storageState` plus a pinned
  `rolter.scope`, so specs start logged in and scoped to the seeded tenant.
  `login.spec.ts` exercises the real login form from a clean state.
- Locate controls by role and label, and take their text from the catalog with
  `t()` from `e2e/i18n.ts` (it reads `src/lib/i18n/locales/en.json`) rather
  than copying English or a placeholder into the spec — hardcoded copy is what
  left seven specs stale after rewordings (#1504). The seeded user is an org
  admin, not a superadmin, so a spec for a superadmin-only screen stubs
  `/api/v1/rbac/effective` the way `mcp-logs.spec.ts` does.

CI: not part of the default PR gate — the `ui e2e (playwright)` workflow runs on
demand (`workflow_dispatch`) and nightly, and uploads traces/screenshots on
failure.

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

### Storybook MCP

`@storybook/addon-mcp` is installed and exposed from the Storybook dev server at
`http://127.0.0.1:6006/mcp`.

For project-scoped agent setup, this repo ships `.mcp.json` at the repo root with
an HTTP server entry named `rolter-storybook` pointing at that endpoint.

The MCP endpoint requires a running Storybook server:

```bash
bun run storybook
```

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
