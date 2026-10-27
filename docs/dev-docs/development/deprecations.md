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

## The release notes

release-plz writes the changelog and cannot call into Rust, so the section is
appended by the `release-plz pr` job: after the release PR is opened,
`scripts/release-deprecations.py` adds the contents of `deprecations.md` to the
newest entry of `crates/rolter-gateway/CHANGELOG.md` (the file the GitHub release
body comes from) and pushes it to the release branch. It adds nothing while the
list is empty and is idempotent.
