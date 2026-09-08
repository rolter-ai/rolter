# Labels

A label is a short, visible fact attached to a provider, a provider group, a
route or a model. Before #985 each such fact was built as a bespoke badge on
one dashboard screen, which meant it existed nowhere else, could not be
filtered on, and could not be added by an operator for their own purposes.
This is the one primitive those facts are built on instead.

## Two kinds, one table

| | `custom` | `auto` |
|---|---|---|
| written by | an operator | a rolter subsystem |
| means | whatever the operator needs — `eu-only`, `prod`, `owner:platform-team` | a fact rolter established |
| provenance | none | `observed_at` and `observation` |
| editable through the API | yes | no |

They share a table because they are rendered and filtered together, and a
screen that showed only one of them would be answering half the question. They
do not share a write path: `LabelRepo::create_custom` and its siblings only
ever touch `source = 'custom'`, and `upsert_auto` / `clear_auto` only ever
touch `source = 'auto'`. Nothing in the repository can turn one into the other.

Two consequences fall out of that, and both are deliberate:

- **An operator cannot forge an observation.** The `labels_provenance` check
  constraint requires an `auto` row to carry `observed_at` and forbids a
  `custom` row from carrying it at all, and no HTTP handler writes `auto`. A
  hand-written label can look like anything except a probed verdict.
- **An auto label never silently overwrites a custom one.** `source` is part of
  the uniqueness constraint, so `(provider X, key "priced")` can hold one of
  each. They are displayed side by side and marked; neither wins.

Editing or deleting an auto label answers `404`, not `403`. The `where
source = 'custom'` clause on the update and delete statements is what enforces
it, so there is no path — not even an internal one reachable from a handler —
that edits an observation.

## An auto label is a measurement, not a guarantee

`observed_at` is the point at which the fact was established and `observation`
is what established it. Both are shown to operators. An auto label says *this
was true when we looked*; it can be stale, and the UI must never present one as
a promise about the next request.

The first producer is the pricing catalog: writing a `model_prices` row records
`priced` on that model with the currency as its value, and deleting the row
withdraws the label. It is written inside the same transaction as the price,
because the moment the fact becomes true is that statement — a background job
would only have to guess how stale it was allowed to be. New producers should
follow the same rule: record the observation where the observed thing changes.

## Subjects

`subject_type` is one of `provider`, `provider_group`, `route` or `model`, and
`subject_id` is text rather than a uuid because a model is addressed by name
while the other three are addressed by id.

That rules out a foreign key, so three `after delete` triggers sweep a
subject's labels when the subject goes. Without them a label would outlive the
row it describes and — worse — be inherited by the next row issued that id.

Tenancy comes from the subject too: the table has no `org_id`, so
`LabelRepo::list_in_org` joins back through `providers`, `provider_groups` and
`routes` to decide what an org may see, and a write checks
`subject_in_org` before it inserts. A label id belonging to another org answers
`404` through this org's path.

## Scopes and the two API surfaces

| Surface | Subjects | Capability |
|---|---|---|
| `/api/v1/orgs/{org_id}/labels` | providers, provider groups, routes | `label` — org-scoped, read `viewer`, write `admin` |
| `/api/v1/model-labels` | models | `model_label` — deployment-wide, read any authenticated caller, write superadmin |

The split follows where the subjects live. Providers, groups and routes belong
to an org; models belong to the deployment-wide pricing catalog and to no org,
so labelling one is a deployment-wide act and `model_label` mirrors
`model_price` exactly.

`label` is its own capability rather than the subject's, so an operator role
can be allowed to annotate a provider without being allowed to re-point it.

## Display and filter only, for now

Labels are not read by the data plane. A route cannot yet *select* on one — a
route that targeted `eu-only` would turn labels into config the gateway
consumes, which is a materially larger change than displaying them.

The schema is shaped so that change needs no rewrite, and the
`labels_bump_config_version` statement trigger is already in place. Bumping
`config_version` on a label write costs a snapshot fetch today and buys the
guarantee that the day a route does select on a label, propagation already
works rather than silently serving stale config (see
[Config & hot reload](config-and-hot-reload.md)).
