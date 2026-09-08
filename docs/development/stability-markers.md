# Stability markers

rolter ships a wide surface, and not all of it is equally finished. Until #1385
nothing distinguished a subsystem an operator could build a deployment on from
one that is a first cut: every screen and every endpoint presented with the same
confidence, so the only way to find the thin parts was to read the source.

A **stability marker** says which parts are still moving. There are two levels:

| Level | Means |
|---|---|
| `stable` | the default. Covered by whatever compatibility promise the release makes |
| `experimental` | **may change shape or be removed in a minor release** |

The source of truth is `SUBSYSTEMS` in
[`crates/rolter-core/src/stability.rs`](../../crates/rolter-core/src/stability.rs).
Nothing re-lists it: the control plane serves it, the dashboard renders it, and
a unit test in that module fails if the table below stops matching.

## What the marker promises

`experimental` is a **documented exemption**, and the exemption is the whole
point. It withholds exactly one guarantee: that the subsystem will still have
the same shape, or exist at all, in the next minor release. Concretely, on an
experimental subsystem a minor release may

- rename, restructure or withdraw its API paths, request and response fields;
- rename or drop its configuration keys and environment variables;
- change what its stored rows mean, or stop reading them;
- remove its dashboard screen.

None of that is licensed on a stable subsystem.

## What the marker does *not* say

This is the half that gets misread, so it is stated positively:

- **It is not a quality warning.** An experimental subsystem is not expected to
  crash, leak or lose data. It is held to the same review, test and security
  standards as everything else. The claim is about the *design* settling, not
  about the code being shaky — which is why the word is "experimental" and not
  "unstable".
- **It is not a support disclaimer.** A bug in an experimental subsystem is a
  bug and gets fixed.
- **It is not a schedule.** Nothing here graduates on a date, and an entry may
  sit for several releases or be withdrawn instead. That is why the word is not
  "beta", which reads as a time-boxed pre-release.
- **It is not "off".** Whether a subsystem runs is a different question
  entirely; see the axes below. Every subsystem in the table is enabled and
  usable in a default deployment.

## The third axis

Stability composes with the two axes ADR-0031 proposes and is deliberately not
a third mechanism inside them:

| Axis | Question | Set by | Where it lives |
|---|---|---|---|
| capability | *can* this deployment run it? | the build and its infrastructure | `unavailable_flags()`, read-only |
| enablement | is it turned on? | the operator | a stored flag, hot-reloaded through `/internal/snapshot` |
| stability | how finished is it? | us, at build time | `SUBSYSTEMS`, a `const` |

The three are independent. An experimental subsystem can be perfectly available
and perfectly enabled, and marking one experimental never turns anything off.

The reason stability is a compile-time constant and not a database row is that
it is a property of the code an operator received. If it were editable it could
be set to a claim the running binary does not support, and it would have to be
propagated, versioned and audited like any other config — for a value that can
only change by upgrading.

ADR-0031 is still proposed at the time of writing, so this page does not depend
on its code. It depends on its vocabulary, which is the part worth keeping
consistent.

## What is experimental in this build

Each entry earns its place by pointing at a gap the tree can be checked
against — a documented "not yet", a management surface the data plane does not
read, an enforcement path a request can take without meeting. Not by
impression.

| Subsystem | Dashboard | Why it is experimental |
|---|---|---|
| `labels` | *(no screen; chips on Providers, Provider Groups and Routing Rules)* | Display and filter only. A route cannot select its targets by label, and making labels selectable turns them into configuration the data plane consumes — a change in what a label *is*. Stated in [Labels](../architecture/labels.md#display-and-filter-only-for-now). |
| `mcp_settings` | MCP → MCP Settings | The screen stores organization defaults for transport, timeout intent, retries, failure policy and undeclared tools; the HTTP proxy still uses deployment-level transport timeouts and does not read them. |
| `mcp_tool_groups` | MCP → Tool Groups | Tool-group manifests are stored and published to MCP-aware clients, but the proxy does not enforce group membership as an access boundary. Access is still decided by virtual-key owner, server, live OAuth session and required scopes. Stated in [MCP OAuth](../architecture/mcp-oauth.md). |
| `realtime` | *(no screen; reachable from the Playground)* | The `/v1/realtime` websocket relay sits outside every request-path subsystem. A session is admitted against process-local caps only and is not metered by budgets, rate limits, guardrails, usage recording or cost attribution, so spend through a realtime session is neither capped nor recorded. |

A subsystem not in that table is stable. The table is short deliberately: a
marker on every page is a marker nobody reads, and the value of this one is
entirely in how rarely it appears.

## Adding, changing or removing a marker

1. Add, edit or delete the `SubsystemStability` row in
   `crates/rolter-core/src/stability.rs`. Keep the list sorted by `id`; the `id`
   is published and is never renamed once it has shipped.
2. Update the table above in the same change. The
   `the_docs_page_lists_exactly_these_subsystems` test compares the two lists
   row for row, so a marker cannot ship without the paragraph that explains what
   it exempts the subsystem from.
3. If the subsystem has a `user-docs/` page, put the note at the top of it, in
   the reader's words rather than these.
4. `nav_keys` are leaf keys from `NAV` in `ui/src/lib/nav.tsx`, checked by
   `every_nav_key_is_claimed_once_and_exists_in_the_dashboard_nav`. Leave it
   empty when the subsystem has no screen of its own; that is normal, not a gap.

**Graduating a subsystem is a deliberate act.** Deleting a row says the shape is
now something we will not change in a minor release, so it belongs in the pull
request that closes the last gap the note names — not in a tidy-up sweep.

## Granularity: why per subsystem

Per endpoint is more precise and rots faster: a route rename silently drops the
marker, and a subsystem's surface is usually several routes plus a screen plus a
config block that all move together. Per dashboard screen cannot express
anything without a screen, which is two of the four entries above.

Per subsystem is the same unit ADR-0031 uses for capability and enablement, so
all three axes answer for the same thing. `nav_keys` then carries the mapping
onto dashboard nav entries, one owner, zero, one or many keys per subsystem.

## Where it is served

`GET /api/v1/version`, alongside the running version and the update check:

```json
{
  "current": "0.1.0",
  "experimental": [
    {
      "id": "realtime",
      "stability": "experimental",
      "note": "the /v1/realtime websocket relay is outside the request-path subsystems: …",
      "nav_keys": []
    }
  ]
}
```

Only the exceptions travel. A subsystem absent from `experimental` is stable,
which is what the dashboard renders nothing for — absence is the signal, and
`"stability": "stable"` is never sent.

It rides on that endpoint rather than one of its own because a marker is a fact
about the build, exactly like `current`; because the endpoint is already covered
by the `version` capability that every authenticated caller holds, so a new one
would only duplicate that row; and because the dashboard shell already fetches
it once per session in the same component that builds the nav, so rendering the
marker costs no extra round trip at app start. `user-docs/api/version.mdx` has
the wire contract, field by field.

## Relationship to the 1.0.0 guarantees (#922)

#922 decides what each *stable* surface guarantees at 1.0 — the `/v1/*` gateway
surface, the `/api/v1/*` control API, the config keys and environment variables,
and the database schema. This page does not write that table and does not
pre-empt it.

What it does is make the table writable. A 1.0 that promises compatibility
across the whole surface either over-promises or is held hostage to the
least-finished corner of the product; naming the exemptions is the honest,
cheap third option. Each row above is one line #922 does not have to argue
about. [API stability and the semver gate](api-stability.md) is the same
argument for the Rust crates.
