# Propagating access-profile model policy to the data plane

**Status:** Accepted · **Date:** 4 Aug 2026 · **Issues:** [#791](https://github.com/rolter-ai/rolter/issues/791), [#534](https://github.com/rolter-ai/rolter/issues/534)
**Relates:** ADR-0022 (config-vs-DB tiering)

## Context

[#534](https://github.com/rolter-ai/rolter/issues/534) added org-scoped access
profiles that can carry a model and route allow/deny policy. The control plane
resolves the profiles a caller holds, merges their policies, and reports the
result on `GET /api/v1/rbac/effective` — but nothing enforced it on the
gateway, so the policy described access it did not actually control.

Enforcement was deferred because the obvious mechanism does not work. The RBAC
tables deliberately carry no `bump_config_version()` trigger: control-plane
authorization is evaluated per request against the live database, so there is
nothing to propagate, and bumping the version on every role edit would wake the
whole gateway fleet for a change it could not observe. Migration `0058` records
that reasoning and states the tables must never grow such a trigger.

The gateway, meanwhile, never sees a user. It authenticates a virtual key and
knows only what the snapshot told it about that key. A policy is a property of
a *person*; a request carries a *credential*. Something has to bridge the two.

## Decision

**Resolve the merged policy per key owner when the snapshot is built, carry it
on the virtual-key record, and enforce it on the gateway's model and route
selection.**

Concretely:

- `rolter_core::ModelPolicy` becomes the single definition of the policy shape,
  its merge rule, and its allow/deny matching. The control plane's
  `MergedPolicy` is now an alias for it.
- `load_virtual_keys` resolves each key's `created_by` user to a merged policy
  in one batched query for the whole key set, and publishes it on
  `VirtualKeyRecord::access_policy`. `None` means unrestricted.
- `KeyMeta::model_permitted` and `KeyMeta::route_permitted` gate the request
  path. The key's own model allow-list and the owner's policy must **both**
  permit a model: they are separate grants and neither may widen the other.
- Migration `0060` adds `bump_config_version()` triggers to exactly the four
  tables that feed this resolution: `access_profile_policies`,
  `access_profile_assignments`, `access_profiles`, and `memberships`.

### Why not resolve at key-mint time

[#791](https://github.com/rolter-ai/rolter/issues/791) floated stamping the
policy onto the key when it is minted. That is unsound: a profile edited after
the mint would never reach keys already issued, so *revoking* a model would not
revoke it. A policy change that fails to restrict is a security bug, and the
failure is silent — the control plane would report the new policy on
`/rbac/effective` while the gateway kept honouring the old one. Snapshot
resolution has the opposite failure mode: the worst case is a bounded staleness
window that the existing config-version machinery already closes.

### Why this does not contradict migration 0058

`0058` says these tables must never bump the config version *because the data
plane does not consume them*. This ADR changes that premise rather than
overruling the rule: the gateway now consumes them, and the repository's
standing convention is that any table the data plane reads must bump the
version inside the write transaction. `0060` therefore applies the rule to the
tables whose premise changed, and only those.

`custom_roles` and `custom_role_grants` stay untriggered. They decide
control-plane authorization, which is still evaluated live per request, so
`0058`'s reasoning holds for them unchanged.

`memberships` is included because it is the second path by which a profile
reaches a user: a profile assigned to a team applies to everyone in it, so
adding a member changes that user's effective policy without any row in the
profile tables changing. Membership is not a hot table, the trigger is
statement-level, and the gateway coalesces on the version number — so even a
bulk SCIM sync costs one snapshot poll rather than one per member.

## Consequences

- The policy `/rbac/effective` reports is now the policy that is enforced. One
  implementation of "deny beats allow" serves the control plane, the store and
  the gateway, so the reported and enforced answers cannot drift.
- A deployment with no access profiles is bit-for-bit unaffected: every key
  carries `None`, and `model_permitted` reduces to the pre-existing
  `model_allowed` check.
- Editing a profile or a team membership now bumps the config version, so those
  edits wake the gateway fleet. This is the intended cost of making the policy
  observable; it is bounded to four tables, none of them on a request path.
- Enforcement is keyed on `created_by`. A key with no owner — admin-created and
  config-defined keys — carries no policy, because there is no person whose
  profiles could apply. Restricting those remains the key's own model list.
- The staleness window is the snapshot poll interval, as it is for every other
  DB-backed config. A revocation is not instantaneous; it is as fast as a
  provider or route change.
