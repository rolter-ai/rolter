# rolter ui

Dashboard for rolter — Vite + React + TypeScript + Tailwind + [shadcn/ui](https://ui.shadcn.com), managed with [Bun](https://bun.sh).

## Develop

```bash
bun install          # install dependencies
bun run dev          # dev server on http://localhost:3000 (proxies /api -> :4001)
bun run build        # production build into dist/ (served by rolter-control)
bun run lint         # typecheck
```

## Add shadcn components

The base config lives in `components.json`. Add components with:

```bash
bunx shadcn@latest add button card table badge dialog input
```

Components are copied into `src/components/ui`. A starter `button` and `card` are already included.

## Structure

- `src/main.tsx` — app entry (React Query + Router)
- `src/App.tsx` — layout + routes
- `src/pages/` — Models, Keys, Logs
- `src/components/ui/` — shadcn components
- `src/lib/api.ts` — typed fetch helpers for the control API
- `src/lib/utils.ts` — `cn()` class helper
