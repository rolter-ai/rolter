# A negotiated format version for `/internal/snapshot`

**Status:** Accepted (28 Oct 2026) · **Date:** 27 Oct 2026 · **Issues:** [#1512](https://github.com/rolter-ai/rolter/issues/1512), [#326](https://github.com/rolter-ai/rolter/issues/326)
**Relates:** ADR-0008 (reload-free config), ADR-0032 (1.0 compatibility guarantees)

## Context

Every gateway started with `--snapshot-url` polls the control plane's
`GET /internal/snapshot` and hot-swaps what it receives (ADR-0008). During a
rolling upgrade the two sides are different builds by definition, and nothing
in the exchange says which shape of payload either one expects. #326 split the
problem in two (scope option A): config-file migrations shipped there, as
`crates/rolter-core/src/config_migrate.rs`, and this record is the snapshot
half, which #326 left as acceptance criteria 1 (snapshot half) and 3.

### What the exchange is today

- **The request** is `GET /internal/snapshot?version=N`. `SnapshotQuery` in
  `crates/rolter-control/src/lib.rs` has one field, `version`, and it does two
  jobs: `build_snapshot` answers `304 Not Modified` when it is current, and
  `cluster::record_heartbeat` stores it as the node's `config_version` in the
  cluster inventory. The gateway also sends `x-rolter-node-build`, but only
  when it can resolve a node id, so the control plane cannot count on knowing
  a gateway's build.
- **The response** is assembled with `json!` in `build_snapshot`, not from a
  shared type: `{"version": N, "config": GatewayConfig}`, plus `problems` when
  the sanitizer dropped something (#926). `version` is the store's
  `config_version`, a monotonic change counter rather than a format version.
  In merged mode `config` can also carry `schema_version`, which is the stamp
  of the control plane's own `rolter.toml` (#326) and says nothing about the
  wire.
- **The gateway** decodes it in `crates/rolter-gateway/src/watcher.rs` into
  `SnapshotResponse { version: u64, config: GatewayConfig, problems }` with
  `resp.json()`. No type in `rolter-core` or `rolter-gateway` carries
  `deny_unknown_fields` (`config_lint` documents why), so an unknown field is
  ignored, and `problems` is `#[serde(default)]` so an older control plane's
  reply still decodes. A decode error or a non-success status makes
  `poll_once` fail: the gateway counts it in
  `rolter_config_reload_failures_total`, logs `config snapshot poll failed`
  (with the body's `error.message` and `error.problems` since #2947), and
  keeps serving the config it last applied.
- **A gateway that has never applied a snapshot** serves its bootstrap file.
  With `--snapshot-url` set it treats an empty key set as closed
  (`managed_auth`), so unless that file carries virtual keys it refuses every
  `/v1` request. Its `/readyz` still answers `200`; the only not-ready answer
  is `draining`.
- **Other readers**, namely `watcher::fetch_log_destination` at gateway start,
  the smoke scripts under `docker/smoke/`, the e2e client and the dogfood
  journey, send nothing but `version`, if that.

### What already goes wrong between two builds

Serde's tolerance is real, but it covers one kind of change, and three other
kinds already bite:

1. **A new value in a closed enum fails the whole decode.** `McpAuthKind` is
   the only enum in the tree with a `#[serde(other)]` fallback (#952).
   `ProviderKind`, `BalancingStrategy`, `BudgetPeriod` and `UnpricedPolicy`,
   among others, have none. So a provider of a kind the newer release added,
   created while older gateways still run, makes every one of them fail every
   later poll. They keep their last config, and with it every virtual key
   revoked since, until they are upgraded or the provider is deleted; and the
   log line reads like any other decode error.
2. **A new restriction is silently ignored.** An older gateway drops a field
   it does not know, which is harmless for a tuning knob and wrong for a rule.
   The user docs already carry the workaround for one case: `upgrading.mdx`
   tells operators to "roll the gateways before you set a cap, or the cap is
   stored and not applied" for route limits (#2936). That is config that
   propagates and is applied in part, which is the failure #1512 names.
3. **A representation can never change.** #1450 made money exact in decimal
   and had to keep it a JSON number on the wire, because a string would have
   broken an older gateway "silently, on the money path". The comment sits on
   `ModelPriceConfig`'s rates in `crates/rolter-core/src/config.rs` and on
   `prices_and_limits_stay_json_numbers_on_the_wire`, which pins the choice.
   (#1512 cites lines 1596 and 4702; both sites have since moved.)

The user docs' advice today is a placeholder: the snapshot "is not yet
version-negotiated. Until it is, upgrade the gateways and the control plane
from the same release". The Helm chart cannot sequence even that: one
`image.tag` drives both Deployments, so a `helm upgrade` rolls the control
plane and the gateways at the same time.

### Why the snapshot refuses where a config file loads

`config_migrate` made the opposite choice for `rolter.toml`. A file stamped
with a `schema_version` from the future loads unchanged with a warning,
because refusing it would turn a rollback into a process that does not start.
The snapshot does not inherit that rule, and the reason is what a refusal
costs. A gateway refused a snapshot keeps serving the config it last applied,
which is the path a `500` already takes today, so a refusal costs staleness
rather than availability. The one gateway it does cost is a gateway that has
never applied a snapshot: it is serving a bootstrap file and refusing every
key anyway, and it should not be taking traffic.

### Constraints

- **The negotiation is itself a rolling-upgrade problem.** The release that
  introduces it talks to builds that predate it in both directions, so it must
  work with a silent peer. Only a later release may depend on the peer
  speaking.
- **ADR-0032 has no rule for `/internal/*`.** Its windows cover `/api/v1/*`,
  the rolter-owned parts of `/v1/*`, and the config file. #1423 built the
  machinery it promised in `crates/rolter-control/src/deprecation.rs`: one
  `DEPRECATIONS` registry drives the `Deprecation` and `Sunset` headers,
  `deprecated: true` in the OpenAPI document and the release-notes section,
  and is held to `MIN_WINDOW_DAYS` and `MIN_WINDOW_MINORS`. #1423 is closed,
  so #1512 is no longer blocked, but its `Target` names an OpenAPI operation or
  schema field, and a snapshot format is neither.
- **The data plane stays lean** (AGENTS.md). Conversion between formats should
  run where the database already is, not in every gateway.

## Decision

### 1. Where the version lives

**In the response, a `format` integer in the envelope:**

```json
{"format": 2, "version": 41, "config": {...}, "problems": [...]}
```

An absent `format` means **1**. Format 1 is defined as exactly the shape the
introducing release serves, which is the shape every earlier build reads. The
field is called `format` rather than `format_version` because the envelope
already has a `version` that means something else, and `config` can carry a
`schema_version` that means a third thing.

**In the request, a `formats` query parameter** stating the inclusive range
the gateway reads, as `formats=2` or `formats=1-2`, beside `version` in
`SnapshotQuery`. It is a query parameter rather than a header because it
qualifies the `304` short-circuit, which already lives in the query; because
it appears in an access log next to `version`; and because every existing
reader can add it with `?`. A value that does not parse is `400` with code
`invalid_query`.

**One format per gateway.** A gateway reads exactly the format its own build
writes, `SNAPSHOT_FORMAT`, and states `formats=N`. The control plane does all
conversion (section 4). The range syntax exists so that letting a gateway read
two formats later would not be a protocol change.

The constants and the range type live in a new
`crates/rolter-core/src/snapshot_format.rs`, shared by both binaries:
`SNAPSHOT_FORMAT` (what this build writes and reads), `MIN_SERVED_FORMAT` (the
oldest format a control plane of this build still serves) and `FormatRange`.

### 2. What counts as a format bump

A change to the payload is **additive**, and needs no bump, when both readers
it can meet behave as they did before it: a newer gateway reading an older
control plane's reply, where the field is absent, and an older gateway reading
a newer one's, where the field is ignored. In addition, ignoring it may make an
older gateway serve _less_ than the operator configured, never more. New
optional tuning fields, new entries in a list, `problems` (#926) and a new value
in an enum whose `Unknown` fallback fails closed (`McpAuthKind` since #952, and
since #3347 every enum that is not deliberately closed) are additive.

Everything else is a **format bump**, which increments `SNAPSHOT_FORMAT`:

1. Removing or renaming a field, or changing its JSON type or representation:
   a number becoming a string (#1450's case), a scalar becoming a list, an
   optional field becoming required.
2. Changing what a field means, its unit, or what its absence means.
3. A new value in an enum the snapshot carries that has no fail-closed
   fallback. When this record was written that was every enum but
   `McpAuthKind`; #3347 gave the others an `Unknown` variant and its reading
   (a provider of an unknown kind is omitted, a route of an unknown strategy
   refuses, an unknown budget period refuses, an unknown failure mode fails
   closed), and #3390 did the same for a route target's `takes_over` class
   (`FailureClass`, whose unknown token matches no failure), so a new
   `ProviderKind`, `BalancingStrategy` or failure class is additive. What
   is left are the enums documented as deliberately closed: `ScopeWithin`,
   `WebhookStage`, `WebhookAuth`, `GuardStage`, `BuiltinRule`, `PluginStage`,
   `DecoratorRole`, `DecoratorPosition`, `PromptTemplateActivationScope` and
   `PolicyScope`. The list and the readings are in
   `docs/dev-docs/architecture/config-and-hot-reload.md`.
4. A new field that restricts what the gateway serves (an access, auth, spend,
   rate, visibility or ingress rule), because an older gateway ignores it and
   serves more than the operator allowed. Route limits (#2936), a key's
   `access_policy` (ADR-0023) and `security.required_headers` would each have
   been one.
5. Any change to the envelope or to the negotiation itself.

A test catches most of case 1: a committed fixture of the current format's
JSON shape (every path a representative config serializes, with its JSON
type), of which the serialized config must remain a superset. A removed path
or a changed type fails until the format is bumped and the fixture is
regenerated with `ROLTER_TEST_UPDATE_SNAPSHOT_SHAPE=1`. Cases 2 to 4 are
judgement, so they go into the AGENTS.md maintenance matrix as a review item:
no test can see a change of meaning, or tell a restriction from a knob.

### 3. Negotiation

**The control plane**, in `build_snapshot`:

1. Records the heartbeat, as today.
2. Reads `formats`. An absent `formats` means a gateway that predates
   negotiation, which reads format 1 and nothing else.
3. Intersects it with `MIN_SERVED_FORMAT..=SNAPSHOT_FORMAT`. If the
   intersection is empty, it answers **`406 Not Acceptable`** with code
   `snapshot_format_unsupported`. This runs **before** the `304`
   short-circuit and before the store is read, so a gateway that cannot be
   served learns it on its next poll even when nothing has changed, at no
   extra database cost.
4. Otherwise answers `304` when `version` is current, as today. Every format
   is built from the same `config_version`, so one counter serves them all.
5. Otherwise builds the snapshot in the highest common format, down-converting
   when that is older than its own (section 4). A down-conversion that cannot
   express the config answers `406` with code `snapshot_format_inexpressible`.
6. Serves it with `format` set, and with `Deprecation` and `Sunset` headers
   when the format served is deprecated (section 5). The `304` of step 4
   carries the same headers, since the format is negotiated before it: a
   gateway whose config does not change for weeks still learns its format is
   going.

Nothing else on `/internal/snapshot` answers `406`. The body follows the
control plane's error envelope, with the explanation repeated in `problems`,
which `refusal_detail` already prints in any gateway built since #2947, so a
gateway that predates negotiation still logs why it was refused:

```json
{"error": {
  "code": "snapshot_format_unsupported",
  "message": "the control plane serves snapshot formats 2-3; the gateway reads 1",
  "problems": ["upgrade the gateway, or roll the control plane back"],
  "served": {"min": 2, "max": 3},
  "requested": {"min": 1, "max": 1}
}}
```

The control plane logs a refusal at `warn` with the node id, its build header
and both ranges, once per node, range and code in each process rather than
once per poll, and records it as a fourth bounded `outcome`, `refused`, on
`rolter_snapshot_build_ms`. A `formats` that does not parse
(`400 invalid_query`) is recorded as `refused` too: it is the negotiation
turning the poll away, not a failure to build.

**The gateway**, in `poll_once` (and, for the first two steps, in
`fetch_log_destination`):

1. Sends `formats=SNAPSHOT_FORMAT` on every request.
2. On a `200`, reads `format` before it decodes `config` (a small struct over
   the same bytes), so that a mismatch is reported as one rather than as a
   serde error. An absent `format` is 1. A format other than its own is
   refused exactly like a `406`: either the control plane predates negotiation
   and ignored the statement, or something served what was not asked for.
3. On a refusal, applies nothing and keeps the config it last applied, as for
   any failed poll today. It sets the gauge `rolter_snapshot_format_refused`
   to 1 and logs at `error` on the transition into that state, with the code,
   both ranges and the action to take; the existing per-poll `warn` stays.
4. On a compatible answer, `200` or `304`, clears that gauge and sets
   `rolter_snapshot_format` to the format of the snapshot it holds (0 while it
   holds only its bootstrap file).
5. On `Deprecation` and `Sunset` headers, logs a `warn` once per process that
   names the sunset date.

**Readiness.** A gateway started with `--snapshot-url` that has **never**
applied a snapshot, and whose last poll was a format refusal, answers
`/readyz` with `503` and the body `snapshot_format_unsupported`. Such a pod is
serving its bootstrap file and refusing every key, so removing it from the
Service costs nothing, and it is what holds a Kubernetes rollout: the new pod
never becomes ready, so the old ones keep serving. A gateway that _has_
applied a snapshot stays ready on a refusal and keeps serving its last config,
because flipping it would turn one out-of-order control-plane upgrade into an
outage of every gateway at once. `rolter_snapshot_format_refused` is the
signal for that case.

### 4. Serving older formats: down-steps in the control plane

The control plane carries a chain of **down-steps**, the mirror image of
`config_migrate::MIGRATIONS`: `DownStep { from: k + 1, to: k, summary, omit,
rewrite }`, applied from `SNAPSHOT_FORMAT` down to the format being served.
`omit` works on the typed config and runs before the sanitizer; `rewrite` works
on the serialized config. `down_convert` runs every crossed step's `omit` and
`down_rewrite` every crossed step's `rewrite`, since the sanitizer sits between
the two. Each step does the first of these that is enough:

1. **Rewrite** the serialized JSON losslessly: rename a field back, render a
   decimal as a number.
2. **Omit** the entities the older format cannot express, such as a provider
   of a new kind or a route carrying a new restriction. The omission runs
   before `sanitize_for_snapshot`, so the existing cascade removes their
   references (route targets, group members, key allow-lists) and writes a
   `problems` line for each, as it does for an invalid provider today. The
   older gateway does not serve the entity at all, which fails closed and is
   named in its log.
3. **Refuse**, as `snapshot_format_inexpressible`, when what the older format
   cannot carry is deployment-wide and there is no entity to omit. The older
   gateways then keep their last config and stop receiving changes, revocations
   included, until they are upgraded. A bump that can be served by omission
   must be.

A step costs nothing while the operator does not use what the bump added: a
route with no cap is not omitted. That is also why the user docs say to finish
a gateway rollout before turning on what a release introduces.

Conversion runs on the process that already builds the snapshot, per poll,
only for an older gateway and only when the version moved. A gateway never
converts.

### 5. The support window

- **A format is deprecated by the release that supersedes it.** That release
  adds an entry to `DEPRECATIONS` with a new target,
  `Target::SnapshotFormat(k)`, so the release notes list it and `problems()`
  holds it to the ADR-0032 window. The snapshot handler, not the route
  middleware, sends `Deprecation` and `Sunset` with every snapshot served in
  that format, since the header depends on the format served rather than on
  the path.
- **A control plane serves every format from `MIN_SERVED_FORMAT` to
  `SNAPSHOT_FORMAT`.** There is no count of formats: several bumps inside one
  window are all served.
- **Raising `MIN_SERVED_FORMAT` past `k` is a removal under ADR-0032 rule 3.**
  It is allowed once format `k`'s entry is past its sunset and the workspace
  version has reached the entry's `removable_from()`: two minor releases after
  the announcing release and 90 days, whichever ends later. A test in
  `rolter-control` holds the constant to the registry. Retiring a format is
  when its down-step may be deleted, and it never happens on its own.
- **This extends ADR-0032 to `/internal/snapshot`**, as a promise between
  rolter's own binaries. A gateway works with a control plane of its own
  release or a later one for as long as that control plane still serves the
  gateway's format: at least two minor releases and 90 days after the format
  was superseded. A gateway newer than its control plane is unsupported; it
  works when the format did not change between the two builds and is refused
  when it did. A script that reads `/internal/snapshot` gets the format it
  states and no other promise.

### 6. The two-release rollout

What each side does with a peer that says nothing:

- **A control plane meeting a gateway that sends no `formats`** serves it
  format 1 while format 1 is served, and answers
  `406 snapshot_format_unsupported` once it is not. It never serves a later
  format to a silent gateway, which cannot say that it reads one.
- **A gateway meeting a reply with no `format`** reads it as format 1. It
  applies it while it reads format 1, and refuses it once it does not.

| Release                             | Control plane                                                                                                | Gateway                                                                                 |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------- |
| N, which introduces negotiation     | writes `format: 1` and serves `1..=1`; a silent gateway gets 1; a later gateway that reads only 2 is refused | states `formats=1`; reads an absent `format` as 1 and applies it                        |
| N+1 or later, the first bump to 2   | serves `1..=2` with a 2→1 down-step; a silent gateway gets 1; announces format 1 deprecated                  | states `formats=2` and **depends on the field**: an absent `format` is 1 and is refused |
| M, at least a window after the bump | serves `2..`, retiring 1, and **depends on the statement**: a silent gateway is refused                      | unchanged                                                                               |

Release N carries no bump on purpose. It is the release in which both upgrade
orders still work, so an operator's first upgrade onto negotiation does not
already need the ordering rule that negotiation introduces; they learn it from
N's release notes. The gateway may depend on the field in the very next
release, because a newer gateway meeting an older control plane is refused
loudly, which is the outcome #1512 asks for. The control plane may depend on
the statement only after a window, because until then a silent gateway is a
supported older build.

N should come as early as possible, since every release before it adds
gateways that can only ever be served format 1. 1.0.0 is not yet tagged (the
newest tag is v0.1.0), so if negotiation makes 1.0.0, every 1.x build speaks
it. That is a scheduling call for the maintainers, not part of this decision.

## Mixed-version behaviour

Each row names the test the implementation adds. In the first PR the control
plane serves only format 1, so the rows that need a second format run against
a served range and a down-step chain passed in as parameters.

**Newer control plane, older gateway**

| Pair                                                                    | Outcome                                                                                                                                           | Test                                                                                                                                                                                       |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| control plane from N on, gateway from before N (silent)                 | served format 1 with `format: 1`, which the old decoder ignores; applied                                                                          | `rolter-control`: the reply to a poll with no `formats` says `format: 1` and decodes through a frozen copy of the pre-negotiation `SnapshotResponse`                                       |
| serves `1..=2`, gateway states 1 or is silent                           | served format 1 through the down-step; anything omitted is in `problems` and in the gateway's log                                                 | `rolter-control`: a test-only chain in which format 2 adds a field and its step removes it; `formats=1` gets no field and `formats=2` gets it                                              |
| serves `2..=2`, gateway states 1 or is silent                           | `406 snapshot_format_unsupported`, also when `version` is current; a running gateway keeps its config with the gauge at 1; a new one is not ready | `rolter-control`: status, body, and precedence over the `304`; `rolter-gateway`: against a stub answering `406`, config unchanged, gauge set, `/readyz` `503` only before a first snapshot |
| serves `1..=2`, gateway states 1, config holds a setting 1 cannot carry | `406 snapshot_format_inexpressible`; the gateway behaves as in the row above                                                                      | `rolter-control`: a test-only step that refuses                                                                                                                                            |
| serves a deprecated format, gateway states it                           | served, with `Deprecation` and `Sunset`; the gateway warns once                                                                                   | `rolter-control`: the headers on that format only; `rolter-gateway`: one warning across several polls                                                                                      |

**Newer gateway, older control plane**

| Pair                                                                          | Outcome                                                                                      | Test                                                                                                                 |
| ----------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| control plane from before N (no `format`, ignores `formats`), gateway reads 1 | read as format 1 and applied                                                                 | `rolter-gateway`: a stub that serves the legacy envelope and ignores the query                                       |
| control plane from before N, gateway reads 2                                  | the absent `format` is 1, so the reply is refused and not applied; not ready if never served | `rolter-gateway`: the same stub, with the gateway's readable format set to 2                                         |
| control plane from N on serving `1..=1`, gateway reads 2                      | `406 snapshot_format_unsupported`; the gateway behaves as in the first table                 | `rolter-control`: the refusal test with the served range as a parameter; `rolter-gateway`: the `406` stub test above |

While the control plane itself is mid-rollout, each replica answers for
itself, so a newer gateway can alternate between applied and refused until the
last old replica is gone. That is why the control plane goes first, and it
needs no test beyond the rows above.

These tests stand in for the older build with fixtures, not with an older
binary. A job in the e2e harness (ADR-0027) that runs the previous release's
gateway image against the new control plane is the real cross-build test. It
belongs in the nightly `extended.yml` and is a follow-up, not part of the
first PR.

## Rolling-upgrade procedure

What `docs/user-docs/deployment/upgrading.mdx` will say, in place of the
"not yet version-negotiated" paragraph:

1. **Upgrade the control plane first, then the gateways.** Every control-plane
   replica, then every gateway. A gateway may lag behind the control plane; it
   may not lead it.
2. **With the Helm chart one `helm upgrade` does this**, even though both
   Deployments move together. When the format moved, a new gateway pod that
   starts before a new control plane is ready answers `/readyz` with
   `503 snapshot_format_unsupported`, so it stays out of the Service and the
   rollout waits while the old pods keep serving. Capacity drops by at most the
   Deployment's `maxUnavailable` while it waits, and `progressDeadlineSeconds`
   may report a stalled rollout that then completes on its own.
3. **Do not turn on what a release introduces until every gateway runs it.**
   An older gateway leaves out the route, key or provider that uses it, and
   names it in its log; for a deployment-wide setting it stops receiving
   changes.
4. **Skipping releases.** A gateway can skip any number. The control plane can
   skip releases as long as the oldest gateway's format is still served; the
   Deprecations section of the release notes lists each format that is going
   and when.
5. **Rolling back** goes in reverse: the gateways first, then the control
   plane, which across a database migration needs the restore that ADR-0032
   rule 5 already requires.

What an operator sees on a mismatch:

- on each refused gateway, one line on the transition, then the existing
  `config snapshot poll failed` warning on every poll:

  ```
  ERROR snapshot format refused: the control plane serves formats 2-3 and this
        gateway reads 1 (snapshot_format_unsupported). Keeping the config applied
        at version 41. Upgrade this gateway, or roll the control plane back.
  ```

- `rolter_snapshot_format_refused 1`, and `rolter_config_reload_failures_total`
  climbing;
- `/readyz` answering `503 snapshot_format_unsupported` on a gateway that never
  received a snapshot, while a gateway that did stays ready on its last config;
- on the control plane, a `warn` naming the node, its build and both ranges,
  and on the Cluster screen the node shown as not converged once the config
  moves past the version it holds.

A gateway built before negotiation, once format 1 is retired, shows only the
line it already writes:
`config snapshot poll failed: snapshot endpoint returned 406 Not Acceptable:`
followed by the message and the `problems` line above.

## Options considered

### Serde defaults alone

The status quo plus review discipline. Rejected: it covers additive changes
and nothing else. It leaves all three failure classes in place, and it makes
every representation permanent, as #1450 found.

### Per-field capability flags

The gateway lists the features it understands (`caps=decimal_money,route_caps`)
and the control plane tailors the payload to that set. Rejected: a capability
set is a format version with 2^n states, each a combination the control plane
must be able to produce and nobody tests. The list only grows, because no point
ever comes at which an old capability stops needing to be listed. The one thing
it buys over a single integer, granularity per entity, the omitting down-step
already provides.

### Content negotiation with `Accept` media types

`Accept: application/vnd.rolter.snapshot+json; format=2`, a `406` on mismatch,
and a `Content-Type` that names the format served. It is the same design in
HTTP's vocabulary, and this record borrows its status code. Rejected for three
practical reasons. The poll's state already lives in the query string and the
`304` is keyed on it, so a choice carried in a header would need `Vary` to stay
correct behind any cache. The payload has to name its format in the body
regardless, so that a captured snapshot is self-describing. And a range in a
media-type parameter is grammar to parse and test, where two integers are not.
An older control plane ignores `Accept` just as it ignores `formats`, so the
transition is no easier either.

### Comparing build versions

The control plane reads `x-rolter-node-build` and refuses a gateway more than
some number of minor releases behind. Rejected: a release is not a format
change. Most releases do not move the format, so two builds several minors
apart may be fully compatible. The header is also sent only when a node id
resolves, and development and fork builds carry versions such a rule cannot
order.

### Gateways that read several formats

Up-conversion in each gateway, so either side could be upgraded first.
Rejected: it puts conversion code into every data-plane process, against the
lean-hot-path rule, to buy an ordering freedom the readiness gate already makes
unnecessary. A gateway rollback needs no such freedom, since an older gateway
with a newer control plane is the supported direction.

### A path per format

`/internal/snapshot/v2`. Rejected: the path is operator configuration
(`--snapshot-url`, the Helm template); `ingest_url()` derives the two push
endpoints by stripping `/internal/snapshot` from it; and a path per format
multiplies routes and OpenAPI rows while telling the control plane nothing
about a gateway that is still on the old path.

### Lock-step: serve only the current format

Refuse anything else, with no window. Rejected: it is correct and loud, but
every bump would freeze every older gateway's config for the length of a
rollout. That is the lock-step upgrade #1512 exists to remove, and it would
leave `/internal/snapshot` with no compatibility at all after ADR-0032 gave
every other surface some.

### Applying a newer format on a best-effort basis

What `config_migrate` does with a file from the future. Rejected: it is
exactly the partial application #1512 rules out, and the argument that
justifies it for a file, that refusing would stop the process, does not hold
for a snapshot (see Context).

## Consequences

- A mixed fleet stops being undefined. Each pair is compatible, or refused with
  a code, a log line, a gauge and a readiness answer.
- Every bump has a fixed cost: the constant, a down-step, a `DEPRECATIONS`
  entry, the shape fixture, and conversion code kept for at least a window.
  New provider kinds and balancing strategies paid it until their enums gained
  a fail-closed `Unknown` fallback in #3347, which made them additive.
- Using a new release's restriction in the middle of a rollout narrows the
  older gateways instead of being ignored by them. That replaces "roll the
  gateways before you set a cap" with an outcome an operator can see.
- A deployment-wide setting that an older format cannot express freezes the
  older gateways' config, revocations included. The design rule (omit where
  possible) and the docs (finish the rollout first) keep that rare; it cannot
  be made impossible without serving those gateways nothing.
- The gateway's `/readyz` gains a third answer. The dashboard's status pill
  (`readinessFrom` in `ui/src/lib/gateway.ts`) read it as unknown until #3350
  taught it the body, which it now shows as down.
- The cluster inventory's `converged` compared `config_version` only, so a
  gateway served an older format with an entity omitted read as converged.
  #3351 records each node's stated range and served format, and a node served
  an older format is no longer converged.
- The channels in the other direction, `/internal/adaptive-telemetry` and
  `/internal/breaker-state`, stay unversioned. They carry telemetry with
  `#[serde(default)]` fields, and a sample an older control plane misreads
  costs a dashboard tile, not a routing decision.

## Implementation

One PR, which makes its release N. It ships format 1 only, so no down-step
exists yet, and test-only steps exercise the chain. Where the shipped code
refines a point above, the section above says so: the `304` carrying the
deprecation headers (section 3, step 6), `400 invalid_query` counting as
`refused`, and the down-step's two halves (section 4). The control plane's
helpers live in `crates/rolter-control/src/snapshot_negotiation.rs` rather than
in `lib.rs`, and the route-limit advice in `upgrading.mdx` is rephrased as an
instance of rule 3 of the procedure rather than removed: it is still true of a
gateway from before #2936, which predates negotiation. The shape guard's
representative config is `crates/rolter-core/tests/fixtures/snapshot_representative.json`,
which gives every list at least one element (a test fails on a list that
serializes empty, since it hides the shape of its items), and the guard ignores
`null`, since an unset `Option` says nothing about the type it carries.

- `crates/rolter-core/src/snapshot_format.rs`, new and exported from
  `lib.rs`: `SNAPSHOT_FORMAT = 1`, `MIN_SERVED_FORMAT = 1`, `FormatRange` with
  parsing and display for `formats`, `negotiate`, the `DownStep` type with an
  empty `DOWN_STEPS`, and `down_convert` with `down_rewrite`. Unit tests for parsing; for
  negotiation with an absent, overlapping, disjoint and malformed statement;
  and for chaining over a synthetic two-step chain that rewrites, omits and
  refuses.
- The shape guard in the same module, its fixture under
  `crates/rolter-core/tests/fixtures/`, and regeneration behind
  `ROLTER_TEST_UPDATE_SNAPSHOT_SHAPE`.
- `crates/rolter-control/src/lib.rs`: `SnapshotQuery::formats`; the
  negotiation ahead of the `304`; `format` in the envelope; the two `406`
  bodies; the `refused` outcome; the once-per-node log; the served range and
  the chain injectable for tests. The control-plane tests of both tables.
- `crates/rolter-core/src/telemetry.rs`: `refused` in the outcome set that
  `record_snapshot` documents.
- `crates/rolter-control/src/deprecation.rs`: `Target::SnapshotFormat`, its
  handling in `problems()` and `render_notes`, a lookup by format for the
  handler, and the test that holds `MIN_SERVED_FORMAT` to the registry.
- `crates/rolter-control/src/openapi.rs`: the `406` on `getSnapshot`, and
  `formats` either documented or named in the `QUERY_UNDOCUMENTED` reason.
- `crates/rolter-gateway/src/watcher.rs`: send `formats`; `format` in
  `SnapshotResponse`; the format-first decode; the refusal state, the
  transition log and the deprecation warning; the readable format as a
  parameter for tests. The gateway tests of both tables.
- `crates/rolter-gateway/src/state.rs`, `metrics.rs` and `handlers.rs`: the
  applied format and the refused flag, the two gauges, and `/readyz`.
  `crates/rolter-gateway/tests/integration.rs` for the readiness answer, since
  the gateway's HTTP surface changes, and
  `docs/user-docs/observability/metrics.mdx` for the gauges, which
  `tests/metrics_docs.rs` checks.
- Docs: `docs/user-docs/deployment/upgrading.mdx` (the procedure above, in
  place of the placeholder and of the route-limit advice),
  `docs/user-docs/community/versioning.mdx` (the promise between the
  binaries), and in `docs/dev-docs/` `architecture/config-and-hot-reload.md`,
  `architecture/config-schema-migrations.md` (its "Not covered here"
  paragraph), `architecture/health-and-readiness.md` and
  `development/deprecations.md`.
- `AGENTS.md`: a maintenance-matrix entry, **Changed the snapshot payload**,
  that says to classify the change under section 2 and, for a bump, lists the
  constant, the down-step, the `DEPRECATIONS` entry and the fixture.

Left out of that PR, each its own follow-up: the dashboard pill (#3350), the
inventory column (#3351), fail-closed fallbacks for the snapshot's enums
(#3347), a per-component image tag in the chart (#3352), and the cross-build e2e
job (#3353). #3347, #3350, #3351 and #3352 have since shipped, together with a
`503 awaiting_snapshot` readiness answer for a gateway that has never applied a
snapshot for any reason (#3348); #3353 is still open.
