# Dashboard capability gating

The dashboard gates its controls on `GET /api/v1/rbac/effective` (#1183).

Before that it gated on nothing. `rg 'useCan|capabilities' ui/src` had zero
authorization hits: every create, edit, delete and toggle rendered for every
signed-in account, and the only feedback was the 403 that came back as a
generic `ApiError` after the click. `lib/scope.ts` said so in its own header —
"scope selection, not permission enforcement". A superadmin-only settings
screen loaded, spun, and then rendered a failure that looked like an outage.

## The rule

**The server decides; the dashboard only repeats the answer early.** The guard
in `crates/rolter-control/src/rbac.rs` still runs on every request, so nothing
here has to be right for the deployment to be safe. This layer only has to be
honest — which is why both uncertain cases fall *open*.

```tsx
<GatedButton gate="provider:create" onClick={() => setSheet({ mode: "add" })}>
  {t("pages.providers.add")}
</GatedButton>
```

`gate` is the `resource:action` pair from `CAPABILITIES` in
`crates/rolter-control/src/rbac_matrix.rs`, spelled exactly as the wire format
spells it, so there is no second vocabulary to keep in step.

## Three answers, not two

`useCan()` returns `boolean | undefined`, and the third one is load-bearing:

| answer | when | what the control does |
| --- | --- | --- |
| `true` | the pair is in `allowed`, or the caller is a superadmin | renders enabled |
| `false` | the caller's role does not reach it | disabled, with the required role in the `title` |
| `undefined` | the query is still in flight, there is no provider above, or the question could not be answered at all | renders **enabled** |

A control that starts disabled and enables itself a request later reads as
broken. And a control plane one version behind — one that 404s
`/api/v1/rbac/effective` — must not empty the dashboard: the 403 stays the
backstop it always was. `ui/src/lib/can.test.ts` pins both.

## What is gated where

- **The rail.** `ui/src/lib/nav.tsx` carries a `resource` per leaf, and
  `visibleNav` drops a leaf whose `<resource>:read` is explicitly `false` — and
  the group with it once it has no children left. A rail full of entries that
  all open the same refusal is a worse map of the product than a shorter rail.
- **A leaf reached by URL.** A hidden entry is still bookmarkable, so `Screen`
  in `App.tsx` renders the `forbidden` `LoadError` up front instead of mounting
  a screen whose every request is already known to fail.
- **The create controls.** Every Add / New / Create / Invite / Generate button
  on the list screens is a `GatedButton` on `<resource>:create`.
- **The per-row controls.** Every edit, delete, retire, revoke, rotate, drain
  and enabled toggle on a row is gated on the same row's
  `<resource>:update` / `<resource>:delete` (#1258). A `Button` becomes a
  `GatedButton`, a `Switch` becomes a `GatedSwitch`, `RowIconButton` takes a
  `gate` prop, and a hand-rolled `<button>` reads `useGate()` at the top of the
  screen — one call for the whole list, because the answer does not vary by
  row.
- **The deployment-scoped settings screens.** Feature flags, the runtime,
  logging, compatibility, client, model-default, adaptive and security policy,
  the cluster, connectors, alerting and the MCP logs are wrapped in
  `superadminOnly()` (`ui/src/components/ForbiddenScreen.tsx`). A non-superadmin
  never mounts them, so they send no request to be refused.

## Disabled has to say why

"Disabled" on its own is the same non-answer the 403 was. `GatedButton` reads
the minimum role out of `GET /api/v1/rbac/matrix` and puts it in the `title`:
`t("rbac.needsRole", { role })`, or `rbac.needsSuperadmin` for a pair no scoped
role can reach.

It stays a **real `disabled`**, never an `aria-disabled` — a control that takes
the click and then explains the 403 has already spent the operator's attention.
The one wrinkle is that the button variants set `disabled:pointer-events-none`,
which also suppresses the native tooltip, so a refused button re-enables pointer
events through an inline style. `disabled` still swallows the click; the
`RefusedSwallowsTheClick` story asserts exactly that.

## One query, per scope

`CapabilityProvider` sits above the shell in `App.tsx` — above, because the rail
is gated too and has to be built with the answer in hand. It runs one query per
org/team/project chain (`staleTime` one minute) plus the matrix for the copy. A
scope switch re-keys the query, so a viewer in one org does not carry a cached
"no" into the next one.

## Adding a screen

Give the nav leaf its `resource`, gate the create control on
`<resource>:create`, gate each row control on `<resource>:update` or
`<resource>:delete`, and wrap the screen in `superadminOnly()` if the capability
table puts it at `scope: "deployment"`. Cover the roles in a story with
`<Harness role="viewer">` — the harness stubs both RBAC endpoints from the same
table this doc names.

## A refused row control still has to name its row

A gate answers "may I", never "which one". The two are separate rules and both
have to hold on the same button: `<resource>:delete` decides whether it is
enabled, and the accessible name decides whether a screen reader can tell it
apart from the eleven identical buttons under it (#1214). So a row control
carries an `aria-label` interpolated with the row's own name —
`t("pages.routing.deleteRoute", { model })`, not `"Delete route"` — and the
`title` falls back to that same sentence when the control is *allowed*, so the
tooltip says what the button does rather than nothing at all.

Naming it with a template literal would type-check and read correctly and still
be wrong: `check:literals` only matches quoted strings, so an
`` aria-label={`Delete route ${model}`} `` slips past the gate while staying
untranslated forever. Route it through the catalogs.

That is also what lets a story select the control it means:

```tsx
await canvas.findByRole("button", { name: "Delete route gpt-4o" });
```

rather than reaching for `getAllByRole(...)[2]` and trusting the confirmation
dialog to prove the right row was picked.
