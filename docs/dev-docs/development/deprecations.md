# Deprecating a control-plane operation or field

[ADR-0032](../adr/2026-09-09-one-point-oh-compatibility-guarantees.md) makes
`/api/v1/*` additive within `v1`: anything the control-plane OpenAPI document
publishes may only be removed after a window of at least **two minor releases and
90 days**, whichever ends later, announced three ways. All three are derived from
one list, `DEPRECATIONS` in `crates/rolter-control/src/deprecation.rs`, so they
cannot drift apart (#1423). The list is empty until the first deprecation.

| Notice                                                  | Driven by                                                                                                                                                            |
| ------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Deprecation` and `Sunset` response headers             | `deprecation::apply`, a layer on the whole control API. The `Deprecation` value is an RFC 9745 date (`@` and Unix seconds), the `Sunset` value an RFC 8594 HTTP-date |
| `deprecated: true` in `GET /openapi.json`               | `Op::to_json` for an operation, `mark_fields` for a property of a `components/schemas` entry                                                                         |
| a `### Deprecations` section in the release's changelog | `render_notes`, checked in as `crates/rolter-control/deprecations.md` and appended by `scripts/release-deprecations.py` on the release PR                            |

## Adding a deprecation

1. Add an entry to `DEPRECATIONS`: the target (an operation by method and path
   template, or one property of a schema plus the operations that serve it), the
   `since` version of the release that publishes it, the `announced` date of that
   release, the `sunset` date, and one sentence on what to use instead.
2. Regenerate the notes: `ROLTER_TEST_UPDATE_DEPRECATIONS=1 cargo test -p rolter-control --lib deprecation`,
   and commit `deprecations.md` with the change.
3. Keep the handler working. A deprecation is a notice, not a removal; the
   target keeps behaving as before for the whole window.

`deprecation::problems` holds every entry to the ADR and fails the build when one
does not meet it: the target must exist in the OpenAPI document, the versions and
dates must parse, a replacement must be named, and the sunset must be at least 90
days after `announced`. The one exemption is `security_fix: true`, which the ADR
allows for a vulnerability and which the release notes then say out loud.

The two-minor half of the window counts releases, which the build cannot see. The
notes print the first release the target may go in (`removable_from`, `since`
plus two minors), and a removal PR is checked against it by review.

## Deprecating a snapshot format

A superseded `/internal/snapshot` format is deprecated through the same
registry ([ADR-0038](../adr/2026-10-27-snapshot-format-version.md) section 5).
The release that bumps `SNAPSHOT_FORMAT` adds an entry whose target is
`Target::SnapshotFormat(k)` for the format it supersedes, so the release notes
list it (as `` `/internal/snapshot format k` ``) and `problems` holds it to the
same window. It names no operation, so `apply` adds nothing for it: the
snapshot handler asks `for_snapshot_format` and sends `Deprecation` and
`Sunset` with every snapshot served in that format, since which format a poll
gets depends on the gateway rather than on the path. `problems` refuses an
entry for format 0 or for a format the build does not supersede.

`MIN_SERVED_FORMAT` in `rolter_core::snapshot_format` is held to the registry
by `min_served_format_is_held_to_the_registry` (`snapshot_format_problems`):
every format from `MIN_SERVED_FORMAT` up to the current one that is
superseded must have an entry, and raising `MIN_SERVED_FORMAT` past `k` (a
removal under ADR-0032 rule 3) fails until format `k`'s entry is past its
sunset and the workspace version has reached its `removable_from`. Retiring a
format is when its down-step may be deleted.

## The release notes

release-plz writes the changelog and cannot call into Rust, so the section is
appended by the `release-plz pr` job: after the release PR is opened,
`scripts/release-deprecations.py` adds the contents of `deprecations.md` to the
newest entry of `crates/rolter-gateway/CHANGELOG.md` (the file the GitHub release
body comes from) and pushes it to the release branch. It adds nothing while the
list is empty and is idempotent.
