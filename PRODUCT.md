# Product

<!-- impeccable:product-schema 1 -->

This file is the product record that design agents read before touching the dashboard (see
[`ui/AGENTS.md`](ui/AGENTS.md)). It holds durable product truth only; the visual system is in
[`DESIGN.md`](DESIGN.md), and the engineering rules are in [`AGENTS.md`](AGENTS.md).

## Platform

web

## Users

The dashboard serves three audiences, and a screen can be opened by any of them. RBAC decides what
each one sees, so a screen must still make sense with half its controls hidden.

- **Platform and infra engineers** deploy and run rolter. They register providers, define routes
  and balancing strategies, pool self-hosted vLLM replicas, and watch health, latency, cache hits
  and errors. They often arrive mid-incident and need the state of the fleet in one look.
- **Org admins and FinOps** own access and money: virtual keys, teams and projects, budgets and
  rate limits, cost attribution, RBAC, SSO and SCIM, the audit log.
- **App developers** hold a virtual key. They try models in the Playground, read their own logs
  and spend, and manage the keys they were given, without operator rights.

Roles are viewer, member and admin, granted at an org, a team or a project, plus the
deployment-wide superadmin. The detailed record is the persona scripts in
[`docs/dev-docs/product/user-journeys.md`](docs/dev-docs/product/user-journeys.md): platform
operator or org admin, team lead, engineer, application or service account, FinOps, SecOps,
DevOps/SRE, and viewer (stakeholder, PM, auditor). Each script names the question that persona
brings to the dashboard.

## Product Purpose

rolter is an OpenAI/Anthropic-compatible AI gateway and load balancer written in Rust. Clients
point at one endpoint; rolter proxies commercial providers and load-balances self-hosted
OpenAI-compatible fleets (for example 20–30 vLLM instances). The data plane (`rolter-gateway`)
serves traffic; the control plane (`rolter-control`) stores configuration and hosts the dashboard.

The dashboard is where rolter is configured and observed. Success means an operator can change
routing, keys or limits and see the change take effect without a restart, and anyone can answer
"what is happening and what does it cost" from the screen in front of them.

## Positioning

- **Fully free, no enterprise edition.** Every screen ships to every user. There are no locked
  tiers, upsell panels or "Enterprise" badges, and there never should be.
- **Cache-aware balancing for self-hosted fleets.** rolter routes prefix-heavy traffic to the vLLM
  replica most likely to have the KV cache warm. This is the mechanism neighbours in the category
  do not have.
- **Rust data plane, runs air-gapped.** Lock-free config reads and minimal-copy streaming on the
  hot path. The whole product, dashboard included, runs with no network access.
- **The full control plane is open source.** RBAC, SSO/SCIM, budgets, rate limits, audit and
  reload-free configuration are all in the open repository.

## Operating Context

- Self-hosted by the team that uses it: a single Docker image (`ghcr.io/rolter-ai/rolter`), a
  native binary (`rolter easy-up`), Docker Compose with Postgres, Redis and ClickHouse, or the Helm
  chart in `charts/`.
- The dashboard is served by the control plane on port 4001; the gateway listens on 4000.
- Deployments are often air-gapped. No runtime request may reach a CDN or external host, so fonts,
  icons and images are vendored.
- Configuration changes propagate to the gateway through `/internal/snapshot` without a reload,
  so the dashboard is a live control surface, not a settings form that waits for a restart.
- Optional backends change what exists: without ClickHouse there are no analytics, without
  Postgres the config is file-backed. Screens must explain a missing precondition instead of
  rendering empty.
- The built-in `fake-llm` model answers locally, so a fresh install can be exercised with no
  provider key.

## Capabilities and Constraints

- Screens cover routing (providers, routes, provider groups, routing rules, complexity router),
  access (virtual keys, users, teams, RBAC, access profiles, SSO, SCIM provisioning), money
  (budgets, limits, pricing, cost attribution), observability (dashboard, logs, performance,
  health, cluster, alerting, audit log), MCP (catalog, management, OAuth, logs), guardrails,
  prompt and skills repositories, plugins, feature flags and the Playground.
- The dashboard is dark-only. There is no light theme and no plan for one.
- Every user-facing string is translated: `en` is the base catalog and `ru` ships beside it, so
  Cyrillic must render everywhere Latin does and a layout must survive the Russian string, which
  is often the longer one.
- Controls are gated by RBAC capabilities from `/api/v1/rbac/effective`. A caller who can act on
  nothing in a region should not be shown that region.
- Destructive actions go through a confirm dialog that names the row and states the consequence.
- Terminology is fixed and used verbatim: provider, route, target, strategy (`round_robin`,
  `weighted`, `cache_aware`, `pipeline`), virtual key, budget, rate limit, org / team / project,
  upstream, gateway, control plane, reload-free.

## Brand Commitments

- The name is lowercase in the product: `rolter`, also at the start of a sentence in UI copy and
  in the browser tab title. Documentation prose may write "Rolter"; the mark and wordmark never do.
- The identity is a folk cross-stitch (вышивка), a nod to the joke the name comes from (rolton
  noodles spliced with router). The mark, its reds, clear space and minimum size are fixed by
  [the brand guidelines](docs/user-docs/community/brand.mdx) and `assets/logo.svg`. The current
  logo is kept.
- The folk layer draws on Slavic embroidery and folklore as a whole, from the Urals to the
  Carpathians. It is not tied to any one country: no national flags, state colours or heraldry,
  and no motif that reads as a political statement.
- Voice is precise and operational. Buttons are imperative verbs ("Add route", "New key"). Screen
  descriptions state what a thing does in one line. Copy leans on concrete nouns and numbers, never
  hype.
- Model names, provider slugs, key prefixes, config keys and money stay in monospace, verbatim.
- No emoji anywhere in product copy or docs.

## Evidence on Hand

- Product copy and scope: `README.md`, `ROADMAP.md`, `docs/user-docs/`, `rolter.example.toml`.
- Who does what, step by step, with the known gaps: `docs/dev-docs/product/user-journeys.md` and
  `docs/dev-docs/product/journeys/`.
- Dashboard rules already written down: `docs/dev-docs/development/dashboard-theme.md`,
  `loading-and-empty-states.md`, `error-states.md`, `destructive-actions.md`, `rbac-gating.md`,
  `form-primitives.md`, `i18n.md`.
- Every screen has a Storybook story with empty, loading and error states; `bun run storybook` in
  `ui/` renders them with faked API responses.
- There are no customer names, testimonials, adoption numbers or published benchmark results.
  Never invent them in UI copy, empty states or marketing surfaces.

## Product Principles

1. **Nothing is paywalled.** A design that implies a tier, a trial or a locked feature is wrong
   for rolter by definition.
2. **State is the content.** Numbers, health and cost are what people come for. Show the real
   state, including "not configured" and "no traffic yet", and say what would change it.
3. **Every role gets a working screen.** Design for the operator, the admin and the developer at
   once, and for the view each of them gets after RBAC removes what they cannot touch.
4. **Offline is the baseline.** Anything the dashboard shows has to work with no network beyond
   the control plane.
5. **Changes are live.** Configuration applies without a restart, so the UI should make clear what
   was saved and when it reached the gateway.

## Accessibility & Inclusion

- WCAG 2 AA is enforced, not aspirational: axe runs after every Storybook story and fails on any
  violation at any impact. Text clears 4.5:1 and shapes clear 3:1 on all four surfaces.
- Full keyboard operation with visible `:focus-visible` rings; focus moves into and back out of
  sheets and dialogs.
- Animations respect `prefers-reduced-motion`.
- Two locales ship today (`en`, `ru`); numbers, money and dates are formatted through the
  dashboard locale, not the browser's.
