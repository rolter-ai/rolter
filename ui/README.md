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
```

## Add shadcn components

The base config lives in `components.json`. Add components with:

```bash
bunx shadcn@latest add button card table badge dialog input
```

Components are copied into `src/components/ui`. A starter `button` and `card` are already included.

## Storybook

Storybook uses the same local Tailwind/design-token stylesheet as the dashboard;
it has no runtime CDN dependency. Add a colocated `*.stories.tsx` file for each
reusable component, export a typed `Meta` object, and include at least one
representative story. Prefer controlled wrappers for interactive components so
their state can be exercised in the Canvas. The first stories live beside
`NavSidebar` and `FilterPanel` under `src/components/ui/`.

## Structure

- `src/main.tsx` — app entry (React Query + Router)
- `src/App.tsx` — layout + routes
- `src/pages/` — Models, Keys, Logs
- `src/components/ui/` — shadcn components
- `src/lib/api.ts` — typed fetch helpers for the control API
- `src/lib/utils.ts` — `cn()` class helper
