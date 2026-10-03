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
honest — which is why both uncertain cases fall _open_.

```tsx
<GatedButton gate="provider:create" onClick={() => setSheet({ mode: "add" })}>
  {t("pages.providers.add")}
</GatedButton>
```

`gate` is the `resource:action` pair from `CAPABILITIES` in
`crates/rolter-control/src/rbac_matrix.rs`, spelled exactly as the wire format
spells it, so there is no second vocabulary to keep in step.

## What `effective` answers per scope

`allowed` is not one role applied to every row. Each capability is decided at
the part of the queried chain its `scope` in `CAPABILITIES` names, which is
exactly the chain its route's guard passes to `authorize`:

| row `scope` | evaluated against    | so a membership held at… reaches it |
| ----------- | -------------------- | ----------------------------------- |
| `org`       | the org alone        | the org only                        |
| `team`      | org + team           | the org, or that team               |
| `project`   | org + team + project | the org, that team, or that project |

A team admin asked at `(org, team, project)` therefore gets `route:create` (a
team-scoped row) but not `team:create` or `custom_role:read` (org-scoped rows),
which the guard would refuse with a 403. Custom-role grants are trimmed the
same way, and `deployment` rows keep the whole chain because they name no
tenancy scope. The `allowed_for_agrees_with_authorize_on_every_row` test in
`rbac_matrix.rs` walks every row against the guard's own decision, so the
advisory answer cannot promise more than the guard grants (#1877).

`provider` and `provider_group` are `project` rows although a row may also be
org-wide (#1919, #2519). The matrix answers for the chain the caller queried, so
a project admin asked at `(org, team, project)` gets the writes `crud.rs` allows
on a provider scoped to that project, and an org admin still passes through the
org membership. The page-level answer cannot tell an org-wide row from a scoped
one: a project admin would see Edit on an org-wide provider and the handler
would answer `403`, because `crud.rs` checks such a row at the org. That check,
not this table, is the authority, so the two list screens gate each row at the
row's own scope (#2522), see
[Gating a row at its own scope](#gating-a-row-at-its-own-scope). Asked at the org alone (no `project_id`) a project
membership reaches neither.

`budget` and `rate_limit` are `project` rows for the same reason (#2527): a caller
whose only role is on a project must read the caps that throttle their own keys,
and a project membership never satisfied an org-scoped read. The row's own scope
is still what the guard checks, so the writes are unchanged: a project admin may
write caps on their project and its keys, never on the team or org above it. The
list routes `GET /api/v1/budgets` and `GET /api/v1/rate-limits` take a
`scope_type` and `scope_id`, and answer `200` for the caller's own project, and for
the team and org above any place they hold a role; another project, a sibling team,
a customer or a business unit answer `403`.

The Roles & Permissions screen (`rbac` in `nav.tsx`) names no resource, so no
capability gates the page: `GET /api/v1/rbac/matrix` publishes what roles can do,
not anyone's data, and answers every signed-in caller. The org's custom roles on
it keep their own check (a role anywhere in that org), and the write controls stay
gated on `custom_role:create`, `:update` and `:delete`.

## Gating a row at its own scope

A capability whose rows may live at more than one scope (`provider`,
`provider_group`) cannot be answered once for the page. Wrap the row's controls
in `RowCapabilityScope` (`ui/src/lib/can.tsx`) and hand it
`rowGateScope(row, orgScope.byTeam)` from `ui/src/lib/provider-scope.ts`:

```tsx
<RowCapabilityScope at={rowGateScope(provider, orgScope.byTeam)}>
  <GatedButton gate="provider:update" control="provider-edit">
    …
  </GatedButton>
  <DeleteIconButton gate="provider:delete" control="provider-delete" … />
</RowCapabilityScope>
```

It swaps the capability context for the controls below it, so the primitives
stay as they are and keep recording their refusals. An org-wide row
(`project_id` null) is asked at the org alone, where a project membership
reaches nothing; a project row is asked at its org + team + project, the team
read from the org's project list. The query key is the provider's, so rows
sharing a scope share one request. A project the dashboard cannot place
(deleted, or not listable by this caller) keeps the page's answer and the `403`
stays the backstop. Stories play a project admin with
`role={adminOfProject(id)}` on `Harness` — a viewer of the org plus an admin
membership on the project — and the stub answers `rbac/effective` for the
queried chain, deciding each row at its own scope as the server does.

Budgets and rate limits (#2529) are the second user. A cap names its own scope,
so `capGateScope(row, { byTeam, keyProjectId })` (`ui/src/lib/limit-scope.ts`)
maps `scope_type` to the chain: `org` is asked at the org alone, `team` at org +
team (`RowScope` carries a `teamId` with no project for it), `project` at org +
team + project, and `virtual_key` at the project the page lists keys of. A
business unit or customer cap, and a project the dashboard cannot place, keep
the page's answer. A project admin therefore edits their project's caps and is
refused the team's and the org's above it, as the guard does.

## Three answers, not two

`useCan()` returns `boolean | undefined`, and the third one is load-bearing:

| answer      | when                                                                                                   | what the control does                           |
| ----------- | ------------------------------------------------------------------------------------------------------ | ----------------------------------------------- |
| `true`      | the pair is in `allowed`, or the caller is a superadmin                                                | renders enabled                                 |
| `false`     | the caller's role does not reach it                                                                    | disabled, with the required role in the `title` |
| `undefined` | the query is still in flight, there is no provider above, or the question could not be answered at all | renders **enabled**                             |

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
  `GatedButton`, a `Switch` becomes a `GatedSwitch`, a `Combobox` becomes a
  `GatedCombobox`, and `RowIconButton` and `DeleteIconButton` take a `gate`
  prop. A screen never gates a control by reading `useGate()` itself and
  passing `denied` down: the control is disabled either way, but only the
  primitives record the reach for it as a `refused_click`, so a hand-gated
  control is invisible to the struggle signal (#1759, see
  [UX telemetry](ux-telemetry.md#why-a-refused-click-takes-a-wrapper)). A
  control none of them fits gets a `gate` prop on the nearest primitive, the
  way `DeleteIconButton` did, and `SwitchRow` did for the project-settings
  switch (#1820).
  `ui/src/components/gated-controls.test.ts` fails on a `useGate()` outside the
  primitives unless the comment above it carries
  `// use-gate-allow: <reason>` — for a genuine non-control use, such as
  `GettingStarted` choosing between a link and a `GatedButton`.
- **The settings cards that save a whole form.** A card whose only mutating
  control is one Save — the sign-in policy on the SSO screen is the model — is
  gated on that resource's `:update` (`org_auth_policy:update`). The fields
  stay readable and editable; the Save is the refusal, because a policy value
  is information a member is allowed to have and blanking the card would leave
  them guessing why sign-in asks for a code (#1078).
- **The workbench header controls.** The prompt repository and the skills
  repository are master-detail screens, so their mutating controls sit in the
  workbench header rather than on a row. Rename, Settings, Save new version,
  Publish and Roll back are all one `<resource>:update` — publishing moves a
  version pointer through the same `PUT` guard as an edit
  (`set_prompt_template_version` / `set_skill_version` in
  `crates/rolter-control/src/crud.rs`), and there is no `:publish` action in
  the capability table to gate on instead — and Delete is the one control that
  takes `<resource>:delete` (#1297). The `AsViewer` story on each screen
  asserts all five are refused and name the Admin role.
- **The first-run checklist.** `GettingStarted` on the Dashboard is written
  for whoever sets the deployment up, so it renders nothing for a caller
  refused all three of `provider:create`, `route:create` and
  `virtual_key:create`, and sends none of its three list requests. The
  Playground step does not count: it takes no role, and one open link above
  three refusals is not a checklist. While the answer is in flight the card
  waits rather than showing and then retracting, and an unanswered gate falls
  open like everywhere else. A 403 from one of its lists hides it too, since
  a role held below the org the provider list is read at is not something
  the gate can say first; it is never a `forbidden` `LoadError` on the first
  screen a member opens (#1848).
- **The pending invitations on the Users screen.** The section reads
  `invitation:read` through `useCan()` for its presence, the way the checklist
  does: it waits for the answer, is absent on an explicit `false`, and sends no
  request then. The control plane filters the list below the org to the teams
  and projects the caller administers, so a `403` from the list is an answer
  too and hides the section instead of raising a `forbidden` `LoadError` on a
  screen a viewer may open. The revoke button is a `RowIconButton` on
  `invitation:delete`; the confirmation still says what revoking at the
  invitation's own scope takes when the server refuses a click the gate allowed
  (#2054).
- **An action a screen takes on its own.** The Playground mints its session
  key as it opens, and minting is `my_virtual_key:create`, which takes the
  member role. So the automatic mint waits for the answer and does not go out
  on an explicit `false`: a viewer lands on the paste field with a line saying
  why, rather than on a refusal they never asked for (#2061). **Mint key** /
  **Renew key** is a `GatedButton` on the same pair. The screen reads
  `useCan()` for the automatic mint only, since that is behaviour rather than
  a control; an unanswered gate still mints, and the `403` stays the backstop.
- **Minting your own key.** **Generate virtual key** on the account screen,
  in the toolbar and again in the empty state, is a `GatedButton` on
  `my_virtual_key:create`, the pair the Playground's mint asks. The empty
  state reads `useCan()` for its copy only: on an explicit `false` it says
  that members mint their own keys and to ask an admin of the project, beside
  the refused button, instead of inviting a mint the button refuses (#2064).
- **Links into a gated screen.** A link the caller cannot follow is a 403
  with an extra click. The LLM Logs payload drawer links to the log settings
  unless `logging_settings:read` is an explicit `false`, the rail's own rule
  for that leaf, and a refused caller reads who owns the setting instead
  (#1984). It is a react-router `Link` rather than a raw anchor, so following
  it does not reload the dashboard; a story that renders it supplies a
  `MemoryRouter`.
- **The deployment-scoped settings screens.** Feature flags, the runtime,
  logging, compatibility, client, model-default, adaptive and security policy,
  the cluster, connectors and alerting are wrapped in
  `superadminOnly()` (`ui/src/components/ForbiddenScreen.tsx`). A non-superadmin
  never mounts them, so they send no request to be refused. The MCP logs are
  scoped on the server since #1831 (`mcp_log:read` is a viewer-level project
  capability); the screen stays wrapped until its dashboard half lands.

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

### The reason is reachable without a pointer

A disabled element takes no focus, so a reason held only in the `title` reached
the mouse and nothing else (#2005). Every gated control (`GatedButton`,
`GatedSwitch`, `GatedCombobox`, `DeleteIconButton`, `RowIconButton`) therefore
sits in `RefusalWrap` (`ui/src/components/ui/refusal-wrap.tsx`). While the
control is refused the wrapper is a focusable `role="group"`, named by the
control it wraps (`aria-labelledby`) and described by the reason
(`aria-describedby`, pointing at visually hidden text beside it), so Tab stops on
it and a screen reader reads "Add provider, group, Requires the Admin role". The
control stays a real `disabled`, and the `title` stays for the mouse. While the
control is allowed the wrapper is `display: contents`, as before. A new gated
primitive wraps itself in `RefusalWrap` rather than hand-rolling a second one.

## Counts, toasts and charts (#2005)

Three other things only some users used to get, fixed in the shared primitives:

- `ListSummary` renders a polite live region (`role="status"`) from the first
  paint, even while empty, so the count a search or filter changes is announced.
- Toasts are paused while the pointer or focus is on the card (WCAG 2.2.1), and
  an error has no timer at all: it stays until dismissed. A caller may still pass
  a `duration`.
- A `LineChart` given a `label` carries a visually hidden table of the values it
  plots, captioned with that label.

## One query, per scope

`CapabilityProvider` sits above the shell in `App.tsx` — above, because the rail
is gated too and has to be built with the answer in hand. It runs one query per
org/team/project chain (`staleTime` one minute) plus the matrix for the copy. A
scope switch re-keys the query, so a viewer in one org does not carry a cached
"no" into the next one.

## The stories render as a role from the real table

`<Harness role="viewer">` stubs both RBAC endpoints, and it derives the answers
from `ui/src/lib/rbac-capabilities.json` — a copy of
`crates/rolter-control/rbac-matrix.json`, which is `GET /api/v1/rbac/matrix`
minus the per-tenant custom roles, rendered by the control plane itself from
`CAPABILITIES`. `ui/src/lib/rbac-capabilities.ts` serves that copy as
`matrixFixture()` unchanged and derives `effectiveFor()` from it the way
`allowed_for` does.

`allowed_for` decides each capability at the part of the queried chain its
`scope` names (`chain_at`, #1877): an org-scoped row at the org alone, a
team-scoped row at org + team, anything else at the whole chain, each with the
role `resolve_role` picks there (most specific membership wins, ties to the
higher role). `effectiveFor()` ports both (#2376), so a team admin's stub
answers `role: "admin"` yet lacks `team:create` and `plugin:create`, exactly as
the server does. A bare `role="admin"` is one org membership, which reaches
every part of every chain; pass memberships to play anyone held lower.
`rbac-matrix-source.test.ts` also re-parses `chain_at`'s match arms and fails
when the port's `CHAIN_TRIMS` disagrees with them.

It used to be a table typed out by hand in `story-harness.tsx`, and nothing
compared the two. So it drifted — #1258 found it calling `model` and
`model_price` org-scoped admin resources when both are deployment-wide catalogs
only a superadmin writes, which let two screens gate on `model:create` and
`model_price:create`, capabilities the control plane does not define, while
their stories passed. A fixture more generous than the deployment makes a
gating story assert behaviour nobody runs.

Two tests keep the copy honest, one per hop:

- `the_checked_in_matrix_artifact_is_what_the_endpoint_publishes` in
  `crates/rolter-control/src/rbac_matrix.rs` renders the matrix through the
  same `builtin_matrix()` the endpoint serves and fails `cargo test` when
  `rbac-matrix.json` differs from it by a byte (the module builds only under
  `--features postgres`, as CI tests it). Run with
  `ROLTER_UPDATE_RBAC_MATRIX=1`, it rewrites the file instead.
- `ui/scripts/rbac-matrix-artifact.test.ts` fails `bun run test` while
  `ui/src/lib/rbac-capabilities.json` is not a byte-for-byte copy of the
  artifact. `bun run gen:rbac` (`ui/scripts/gen-rbac-capabilities.ts`) makes
  the copy.

**Change the capability table, run `just gen-rbac` and commit both JSON files
with it.** The recipe does both hops. Until #1369 the generator parsed the Rust
source for the table, which tied it to the exact shape of a `const` struct
literal; the artifact is the control plane's own rendering, so the table can
be reorganised freely (#1298).

## Adding a screen

Give the nav leaf its `resource`, gate the create control on
`<resource>:create`, gate each row control on `<resource>:update` or
`<resource>:delete`, and wrap the screen in `superadminOnly()` if the capability
table puts it at `scope: "deployment"`. Cover the roles in a story with
`<Harness role="viewer">` — the harness stubs both RBAC endpoints from the same
table this doc names.

## Never hold a control across a re-render

`expectRefused` and `clickWhenEnabled` look their button up again on every poll
rather than capturing it once, and a story that asserts on a gated control by
hand has to do the same.

A screen re-renders while its gate is still in flight: the org/team/project
chain resolving re-keys the query behind the screen, which sends it back to its
skeleton for a frame, and React builds a _new_ button when it returns. A
reference taken before that frame is detached, and a detached node's attributes
never change again — so the assertion waits out its whole budget and reports
`title: null`, which reads as a gate that never resolved even though the live
control is refused correctly.

That is what #1670 was: `Screens/Rbac › RefusedToAViewer` failed identically at
50ms of injected latency and at 900ms, and raising the budget from 1s to 5s did
nothing. `Harness/Gating` stages the ordering deliberately — the control is
replaced _before_ the answer it is waiting for arrives — so the two helpers
cannot regress to a captured reference.

```tsx
// wrong: the reference can be detached before the gate answers
const button = await canvas.findByRole("button", { name: /new role/i });
await waitFor(() => expect(button).toBeDisabled());

// right: the lookup is part of the wait
await waitFor(() => {
  expect(canvas.getByRole("button", { name: /new role/i })).toBeDisabled();
});
```

## A refused row control still has to name its row

A gate answers "may I", never "which one". The two are separate rules and both
have to hold on the same button: `<resource>:delete` decides whether it is
enabled, and the accessible name decides whether a screen reader can tell it
apart from the eleven identical buttons under it (#1214). So a row control
carries an `aria-label` interpolated with the row's own name —
`t("pages.routing.deleteRoute", { model })`, not `"Delete route"` — and the
`title` falls back to that same sentence when the control is _allowed_, so the
tooltip says what the button does rather than nothing at all.

Naming it with a template literal would type-check and read correctly and still
be wrong: `check:literals` only matches quoted strings, so an
``aria-label={`Delete route ${model}`}`` slips past the gate while staying
untranslated forever. Route it through the catalogs.

That is also what lets a story select the control it means:

```tsx
await canvas.findByRole("button", { name: "Delete route gpt-4o" });
```

rather than reaching for `getAllByRole(...)[2]` and trusting the confirmation
dialog to prove the right row was picked.
