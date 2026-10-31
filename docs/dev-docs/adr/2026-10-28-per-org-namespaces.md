# Per-org namespaces for routes, providers and provider groups

**Status:** Accepted (28 Oct 2026) · **Date:** 28 Oct 2026 · **Issues:** [#1857](https://github.com/rolter-ai/rolter/issues/1857), [#1844](https://github.com/rolter-ai/rolter/issues/1844), [#1845](https://github.com/rolter-ai/rolter/issues/1845)
**Relates:** ADR-0008 (reload-free config), ADR-0017 (provider/model addressing), ADR-0038 (snapshot format version)

## Context

In the store, routes, providers and provider groups all belong to an org:
routes through their project, providers and groups directly. The tables
already say so: `providers` is unique on `(org_id, name)` and `(org_id, slug)`,
`provider_groups` on `(org_id, slug)`, `routes` on `(project_id, model)`. Every
row the control plane puts in `/internal/snapshot` carries that owner as
`tenancy`, and the gateway already refuses a row to a key of another org
(#1844).

The gateway's maps are keyed by bare name, across the whole deployment:

- `Snapshot::routes` by public model name;
- `Snapshot::providers` by provider name, and `providers_by_slug` by slug;
- `Snapshot::groups_by_slug` and `group_balancers` by group slug.

So is the gateway's runtime state. The load tracker (in-flight counts, the
`fastest` latency window, the latency predictor), cooldowns and circuit
breakers are keyed by `(route namespace, target index)`, where the namespace is
the requested model or `model::variant`. Key pools, provider queues, scraped
queue depth, health probes and KV/LMCache telemetry are keyed by provider name.

Two orgs that both want a `gpt-4o` route or an `openai` provider therefore
collide. Before #1845 the second row made `validate_snapshot` refuse, which
froze config propagation for the whole fleet. The tenancy PR (#1859, landed
with #1842) closed that by refusing the colliding write with `409`
(`RouteRepo::model_in_use`, `ProviderRepo::name_in_use`, the deployment-wide
`slug_in_use` / `address_slug_in_use`, and the same rule in
`rolter-seed --import`). That is safe, but it puts every tenant in one
namespace and tells an org what another org has named its rows. MCP servers
never had the problem: `mcp_servers` has been keyed `(org_id, slug)` from the
start.

## Decision

### 1. The naming model

A route, provider or provider group is named by **`(org, name)`**, where `org`
is `tenancy.org_id` and `name` is the route's `model`, the provider's `name` or
slug, or the group's slug. A row with no `tenancy` comes from the gateway's own
`rolter.toml` (or the control plane's bootstrap file): it belongs to the
deployment and sits in the **global** namespace, written `org = ""`.

Names are unique per namespace, never across them:

- two orgs may each have a `gpt-4o` route, an `openai` provider and an `eu`
  group;
- one org may not have two `gpt-4o` routes, even in different projects, because
  a key resolves names per org, not per project; nor two providers or groups
  answering one slug;
- an org row may share a name with a global row.

A key resolves a name in **its own org first, then in the global namespace**.
An org row therefore shadows a global row of the same name for that org's keys
only. A key with no org (one from the gateway's own config file, which is the
operator's) resolves the global row; with no global row it resolves the org row
only when exactly one org owns the name, and treats a name two orgs own as
unknown, rather than guessing. Inside the config the rule is stricter: a
route's targets, a variant's targets, a group's members and a route's
semantic-cache provider name a provider that resolves in the row's own org
first, then globally, and nothing else (`Namespaced::resolve_ref`). A row never
resolves another org's provider, so the sole-owner fallback a key gets does not
apply to a row: a global route naming a provider only one org has would
otherwise hand that org's credential to every org's key. The sanitizer prunes
such a target with a line in `problems`, and the gateway rewrites any name
that resolves to nothing to an id no provider has.

Resolution stays what #1844 made it: a route, address or builtin is tried in
the same order as before (a named route, then `provider-slug/model`, then
`group-slug/model`), each lookup scoped as above, and a name that belongs to
someone else is a miss rather than a refusal.

### 2. Snapshot maps

`Snapshot` keys its tables by name and then by org, so a lookup is two borrowed
`&str` probes and allocates nothing:

- `routes` becomes a `RouteTable`: model name, then one entry per org plus an
  optional global one;
- `providers_by_slug` and `groups_by_slug` become the same two-level shape,
  with each group carrying its own balancer (which replaces the separate
  `group_balancers` map);
- `providers` stays a flat map, keyed by the provider's **runtime id**
  (section 3), which is also what every route target, variant target and group
  member carries once the snapshot is built.

The tables are built once per snapshot and swapped through the existing
`ArcSwap`; nothing on the request path takes a lock to read them.

### 3. Runtime ids and per-provider state

Every piece of state that outlives a request is keyed by a runtime id, so two
same-named rows in two orgs never share it:

- a **provider's runtime id** is its name when no other provider in the
  snapshot has that name, and `name@org_id` when one does (the global row, if
  there is one, keeps the bare name). Snapshot build rewrites every reference
  to the provider (route and variant targets, group members) to the runtime id,
  after resolving it in the referencing row's org;
- a **route's namespace** (`RouteEntry::namespace`) follows the same rule on
  its model name, and a synthetic `provider-slug/model` or `group-slug/model`
  route gets `address@org_id` when the provider or group it pins is
  qualified.

Breakers, cooldowns and the load tracker (in-flight counts, the latency
window, the predictor) are keyed by `(namespace, target index)` and
`variant_key(namespace, variant)`, so they follow the route namespace. Key
pools, provider queues, scraped queue depth, health probes and KV/LMCache
telemetry are keyed by the provider's runtime id. None of those stores changes
type: each already took an opaque string, and the snapshot now hands it one
that cannot be shared across orgs. That also keeps the breaker's admission
logic (#3307) untouched.

Qualifying only on a collision is deliberate. A deployment with no repeated
name, which is every deployment today, keeps every key, metric label and log
value it has now. The cost is that the first time a second org takes a name,
the first org's runtime id for it changes from `openai` to `openai@<org>`: its
breaker, cooldowns, latency window and queue start fresh once, and its
Prometheus series continue under the new label. That is one reset of soft
state, on an event that is rare and visible in the audit log, against renaming
every series of every control-plane deployment on upgrade. A provider name is
free text and could itself contain `@<uuid>`; snapshot build checks that a
provider's runtime id and a route's namespace are unique and leaves out (with
a warning) a row whose id would still collide.

The response cache is the exception to qualifying only on a collision: a
route that belongs to an org keeps its exact-match entries and its semantic
index under that org whether or not another org shares the name, and a global
route keeps the key it had. Cached answers outlive the routes that wrote them
(a route can set a long TTL), so a key that changed with the sharing would let
an org's entries, written while its name was unshared, answer a later route of
the same name in another org once the first route is gone. The price is one
cold cache for org routes on upgrade.

### 4. What logs, metrics and health events record

Log rows and health events keep the **display name** in `provider` and record
the owner separately, so a reader never has to parse a runtime id:

- request logs gain `provider_org_id` (ClickHouse `037`), empty for a global
  provider. `org_id` stays the key's org; the two differ when a key of an org
  is served by a global provider;
- `provider_health_events` already has `org_id`, stamped from the provider
  list on each reload (#1842); it is now stamped by runtime id, so two orgs'
  `openai` probes are two series with the right owner each, and #1842's
  provider-health filter, which matches on `org_id`, keeps working unchanged;
- the `x-rolter-provider` response header and the GenAI span's
  `gen_ai.provider.name` carry the display name;
- Prometheus labels carry the runtime id, which is what tells two orgs' series
  apart; they are operator-facing.

### 5. The snapshot, under ADR-0038

The wire shape does not change: `tenancy` is already on every route, provider
and group, so the shape fixture is untouched. The meaning does: a payload may
now carry two rows with one name. Under ADR-0038 section 2 that is a change of
meaning (case 2). It is **not** a `SNAPSHOT_FORMAT` bump, because format 1's
meaning is fixed when 1.0.0 is tagged, negotiation itself (#1512) has not been
in a release yet, and no format bump is made before 1.0.0. Format 1 is defined
as the keyed snapshot from this change on.

Two kinds of gateway can meet such a payload:

- **A gateway that predates negotiation** (it sends no `formats`) still
  refuses a duplicate name, and would freeze. The control plane folds the
  namespaces for it: before sanitizing, every org-owned route, provider or
  group whose name another row of its kind also uses is left out, each one
  named in `problems`, and the cascade prunes what referenced it. Leaving all
  of them out rather than picking one fails closed: those orgs' keys miss the
  name on that gateway until it is upgraded, and no key is ever served another
  org's row. A global row is kept.
- **A negotiating gateway built before this change** states `formats=1` and
  cannot be told apart. Such builds exist only between #1512 and this change on
  `master`, never in a release, so they are not served a folded snapshot.

### 6. The write-time 409 goes

With collisions harmless, the deployment-wide checks from the tenancy PR are
removed: creating or renaming a route, provider or group refuses only on a
name or slug **its own org** already uses (`model_in_use_in_org`,
`name_in_use_in_org`, and the slug checks scoped by `org_id`), and
`rolter-seed --import` applies the same rule. Two refusals stay. A route whose
name sits on another org's `provider-slug/model` address is still refused: it
is about the operator's org-less keys reaching an address, not about
collisions. And a name or slug a readonly row of the control plane's
bootstrap file holds is still refused to every org's rows, as it was before
this change: those rows are the operator's, and the dashboard cannot edit
them, so an org row shadowing one for its own keys would be a change the
operator never sees. A gateway-only `rolter.toml` has no such guard, since it
has no store to write to; there an org row of the same name does shadow the
global one for that org's keys, as section 1 says.

## Options considered

### Prefixing names with the org on the wire

The control plane could send `org-a/openai` as the provider name. Older
gateways would accept it, but every log line, metric and header would carry
the prefix, keys' allow-lists and route targets would have to be rewritten in
the store's terms, and a route's public name cannot be rewritten at all,
because it is what the client sends.

### Always qualifying org-owned rows

Stable ids from day one, at the price of renaming every runtime key and
Prometheus series of every control-plane deployment on upgrade, and of putting
an org uuid in every label where today there is a readable name. Rejected for
the collision-only rule of section 3.

### Changing the state stores' key types to `(org, name)`

Every store (breakers, cooldowns, load tracker, queues, key pools, health,
telemetry) would take a tuple. It touches every call site and the breaker's
admission logic, for the same isolation the runtime id gives with no type
change.

### Keeping the 409

Safe, and what shipped as the interim fix, but it keeps one namespace for all
tenants and leaks the existence of another org's names through the refusal.

## Consequences

- Two orgs can each create a `gpt-4o` route and an `openai` provider; each
  org's keys reach only their own, the snapshot serves both, and a failing
  provider in one org never opens the other's breaker.
- An org that creates a name another org already uses resets that org's soft
  state for the name once (section 3).
- An operator key with no org loses access to a name as soon as a second org
  takes it and no global row exists; operators who route through org rows
  should mint their keys in an org.
- Model prices stay keyed by model name across the deployment; per-org prices
  are out of scope.

## Implementation

- `rolter_core::namespace`: the `(org, name)` resolution shared by validation,
  the sanitizer and the gateway, plus the legacy fold of section 5;
  `validate`/`sanitize_for_snapshot` check uniqueness and resolve references
  per org.
- `crates/rolter-gateway/src/state.rs`: runtime ids, `RouteTable`, the scoped
  slug tables and `RouteEntry::namespace`; the request paths read the
  namespace instead of the requested model for loads, cooldowns and breakers.
- logging and health events: display name plus `provider_org_id` / `org_id`.
- `crates/rolter-control` and `crates/rolter-store`: the org-scoped write
  checks, the seed, and the fold for a silent gateway in `build_snapshot`.

## Addendum (29 Oct 2026) — an org row the bootstrap file later repeats ([#3516](https://github.com/rolter-ai/rolter/issues/3516))

Section 6's refusal is a **write-time guard only**: it stops an org row being
created, or renamed, onto a name a readonly row of the control plane's bootstrap
file holds. It does not make the file's row win in the effective config. Two
states still reach the store with such a pair: an org row created before the
operator added the same name to the file, and a row written around the API (by
SQL, for example).

`MergedConfigStore::load` used to drop every stored provider, route or provider
group whose name (a group's slug) any file row used, whatever the row's tenancy.
The org's keys then resolved the global row in silence: other credentials,
another `api_base`, other targets. And the per-org config export (#3481) never
saw the org's row, so it wrote the file's in its place, and re-importing that
file overwrote the org's own row.

Decided: the effective config follows section 1. A stored row is dropped only
when a file row of the same kind has the same name **in the same namespace**
(`(org, name)`, the org of a row with no tenancy being the global one). An
org-owned row stays and shadows the file's for that org's keys only; global keys
and every other org still resolve the file's row. A stored global row the file
repeats is still dropped, as before.

The overlap is reported, as a warning rather than an error, under
`GET /api/v1/config/problems`: one line per org row, naming the org, the kind and
the name, and saying that the org's row shadows the readonly file row for that
org. `MergedConfigStore::load_problem_records` adds it, owned by the org's
tenancy, so a caller who cannot see that org does not see the line. The per-org
export writes the org's row, so a re-import leaves it as it was.

The overlap is also counted, as `kind = bootstrap_shadow`, in the control
plane's `rolter_config_problems{kind}` gauge, sampled when it builds a snapshot
(#3542).

The control plane's config-owned guards follow the same key. An org's row that
shares a name with a global file row is the org's: its admin edits and deletes
it, and `GET /api/v1/models` lists the org's route as stored (`source: "db"`)
and only the file's as `config`. Creating such a row stays refused (section 6).

A key used to resolve a route or a pinned address by name in its own org first
and only then check the project, so an org's project-narrowed shadow (a
`project_only` route, or a project-scoped provider or group) left the org's
other projects with the unknown-model answer instead of the file's row. That
failed closed, and is revised by the addendum below (#3546).

Keeping the drop and only reporting it was considered and rejected: the gateway
would keep answering one org's traffic with another row's credentials and
targets, and the warning would only say so after the fact.

## Addendum (29 Oct 2026) — a project-narrowed org row falls through ([#3546](https://github.com/rolter-ai/rolter/issues/3546))

Decided: an org's row the key may not use is no match for that key. A
project-narrowed org row (a `project_only` route, or a provider or group scoped
to one project) shadows the global row of its name only for the project it
serves. A key of any other project of that org resolves the name as if the org
had no row of it, and so reaches the global row, under that row's own tenancy
check. A key of another org was already served the global row and still is.

Nothing is widened: the key reaches only the global row, which it could use
before #3516 made the org's row visible, and only when that row's own tenancy,
visibility and access rules admit it. An org that wants a model kept from its
other projects says so on the global row's own rules; a project-narrowed shadow
was never meant to do that job.

Section 1's order is unchanged; the project check moves inside it. Every path
that resolves a name the way a request does gives the same answer:

- `Snapshot::named_route_for`, and through it the realtime upgrade, tier
  routing and the builtin's shadow check, resolve a route with
  `Namespaced::resolve_admitted`, which goes on to the global row when the
  org's own row fails `RouteEntry::in_tenancy_of`;
- `Snapshot::resolve_for` retries a pinned `slug/model` address against the
  global provider and group of the slug when the org's own one is outside the
  key's project;
- `GET /v1/models` lists the route, provider and group each key resolves to by
  the same rule, so a sibling project lists the global rows its requests reach.

MCP servers are not part of this: the gateway resolves a server by the key's own
org and slug only, and has no global row to fall through to.
