#!/usr/bin/env bash
# the release pipeline hands off in two hops, and both are easy to delete by
# accident because neither has a test behind it:
#
#   release-plz.yml  --workflow_dispatch-->  release.yml  --> wheels/pypi/ghcr
#   release-plz.yml  --workflow_dispatch-->  ci.yml      --> ci-ok on the release pr
#
# release-plz tags with the repo GITHUB_TOKEN, and GitHub suppresses downstream
# events for token-created refs, so release.yml's `push: tags` trigger never
# fires for a real release. `workflow_dispatch` is the documented exception —
# it always creates a run. drop that dispatch and releases keep going out with
# a github release and crates.io but no wheel, silently and forever: that is
# exactly what happened to v0.0.6-v0.0.10 while pypi sat on 0.0.5 (#903).
#
# this check asserts the handoff and its parity gate are still wired up.
set -euo pipefail

plz=".github/workflows/release-plz.yml"
rel=".github/workflows/release.yml"
ci=".github/workflows/ci.yml"
fail=0

require() {
    # $1 file, $2 human description, $3 grep pattern
    if ! grep -Eq "$3" "$1"; then
        echo "error: $1: $2" >&2
        fail=1
    fi
}

forbid() {
    # $1 file, $2 human description, $3 grep pattern that must NOT match
    if grep -Eq "$3" "$1"; then
        echo "error: $1: $2" >&2
        fail=1
    fi
}

for f in "$plz" "$rel" "$ci"; do
    if [[ ! -f "$f" ]]; then
        echo "error: $f is missing; the release pipeline needs every one of them" >&2
        exit 1
    fi
done

# ── the handoff itself ──────────────────────────────────────────────────────
require "$plz" "no 'dispatch-artifact-release' job — release.yml will never run" \
    '^  dispatch-artifact-release:'
require "$plz" "the handoff job no longer dispatches release.yml" \
    'gh workflow run release\.yml'
require "$plz" "the handoff job needs 'actions: write' to dispatch" \
    '^      actions: write'
require "$plz" "the handoff must run after release-plz-release, on its tag output" \
    'needs: release-plz-release'
require "$plz" "release-plz-release must expose the resolved tag as a job output" \
    '^      tag: \$\{\{ steps\.tag\.outputs\.tag \}\}'

# ── the tag the handoff carries ─────────────────────────────────────────────
# release-plz emits a `tag` for every published crate, not just the one with
# git_tag_enable: the crates.io-only members get a derived `<crate>-v<version>`
# string that was never pushed. picking the first tagged entry resolved
# `rolter-core-v0.0.11`, release.yml checked out a ref that does not exist, and
# v0.0.11 shipped to crates.io with no wheel (#1026). select by package name and
# refuse anything that is not a vX.Y.Z tag.
require "$plz" "the tag must be selected by package_name, not by array position" \
    'select\(\.package_name == "rolter-gateway"\)'
require "$plz" "the resolved tag must be validated as a vX.Y.Z release tag" \
    "grep -Eq '\\^v\\[0-9\\]"
forbid "$plz" "do not take the first tagged release; crates.io-only tags are never pushed" \
    'map\(select\(\.tag != null and \.tag != ""\)\) \| \.\[0\]'

# `gh api` switches to POST as soon as any -f/-F is present, which 404s the
# GET-only workflow-runs endpoint and aborted the confirmation loop (#1026)
forbid "$plz" "the run-confirmation query must be in the path; -f makes gh api POST" \
    '^ *-f event=workflow_dispatch'
require "$plz" "the handoff must confirm a release run actually started" \
    'actions/workflows/release\.yml/runs\?event=workflow_dispatch'

# ── gating the release pr ───────────────────────────────────────────────────
# the same suppression hides the release *pr* from ci.yml: `ci-ok` is the only
# required check on master and nothing ever reported it there, so every release
# merged through an admin bypass (#1025). drop this dispatch and releases go
# back to needing one, quietly.
require "$plz" "no 'dispatch-release-pr-ci' job — the release pr will never get ci-ok" \
    '^  dispatch-release-pr-ci:'
require "$plz" "the release-pr gate no longer dispatches ci.yml" \
    'gh workflow run ci\.yml'
require "$plz" "the release-pr gate must run after release-plz-pr" \
    'needs: release-plz-pr'
require "$plz" "the release-pr gate must confirm a ci run actually started" \
    'actions/workflows/ci\.yml/runs\?event=workflow_dispatch'
require "$ci" "ci.yml dropped its workflow_dispatch trigger; the release pr cannot be gated" \
    '^  workflow_dispatch:'
require "$ci" "ci.yml must still aggregate everything into the ci-ok check" \
    '^  ci-ok:'

# ── the receiving end ───────────────────────────────────────────────────────
require "$rel" "release.yml dropped its workflow_dispatch trigger" \
    '^  workflow_dispatch:'
require "$rel" "release.yml dropped the 'tag' dispatch input" \
    '^      tag:'
require "$rel" "release.yml no longer has a publish-pypi job" \
    '^  publish-pypi:'
# the gate is asserted, not re-run. calling quality.yml from here would check out
# the *caller's* ref — master on a dispatch, not the tag being packaged — so the
# gate would verify one tree while the wheels came from another (#988). worse,
# threading the tag in instead makes the shared workflow check out a
# dispatch-supplied ref in a default-branch context, which is cache poisoning.
forbid "$rel" "release.yml must not re-run quality.yml; assert ci-ok for the tagged sha instead" \
    '^ *uses: \./\.github/workflows/quality\.yml'
require "$rel" "the release gate must require ci-ok for the tagged commit" \
    'RELEASE_REQUIRED_CHECKS \|\| .ci-ok,'

# ── the parity gate ─────────────────────────────────────────────────────────
# without this a skipped publish drags the run to green instead of red
require "$rel" "release.yml dropped the verify-parity gate" \
    '^  verify-parity:'
require "$rel" "verify-parity must observe publish-pypi to catch a skipped publish" \
    'needs: \[verify-external-checks, build-wheels, build-image, smoke-wheels, smoke-image, publish-pypi, publish-docker\]'
require "$rel" "verify-parity must run with always() or a skipped publish stays invisible" \
    'if: always\(\)'

# ── the artifact set ────────────────────────────────────────────────────────
# `macos-latest` is arm64, so without an explicit x86_64 target intel macs get
# no wheel; without an sdist they get no candidate at all and the install fails
# outright rather than building from source (#989)
require "$rel" "release.yml must cross-build the macos x86_64 wheel" \
    'target: x86_64-apple-darwin'
require "$rel" "release.yml must build an sdist as the source fallback" \
    '^          command: sdist'
require "$rel" "verify-parity must assert the published artifact set" \
    'expect "macos x86_64 wheel"'
require "$rel" "verify-parity must assert an sdist was published" \
    'expect "sdist"'

# ── the publish barrier ─────────────────────────────────────────────────────
# stage 1 builds, stage 2 smoke-tests, stage 3 publishes. if a publish job stops
# depending on every build and smoke job, a failed wheel can leave images public
# against a release with nothing on pypi (#992)
require "$rel" "release.yml must build images per arch in their own stage" \
    '^  build-image:'
require "$rel" "stage 1 must push untagged digests, not tags" \
    'push-by-digest=true'
require "$rel" "the publish jobs must wait for every build and smoke job" \
    '^    needs: \[verify-external-checks, build-wheels, build-image, smoke-wheels, smoke-image\]'

# an artifact that was never run is not a verified artifact
require "$rel" "release.yml must install and run the built wheel before publishing" \
    '^  smoke-wheels:'
require "$rel" "release.yml must run the built image before publishing" \
    '^  smoke-image:'
require "$rel" "the wheel smoke test must resolve only from dist/, never pypi" \
    'no-index'

if [[ "$fail" -ne 0 ]]; then
    cat >&2 <<'EOF'

the release handoff is broken. see docs/dev-docs/development/packaging.md ("Release
pipeline"); a release that loses this wiring publishes a github release and
crates.io but never a pypi wheel, or leaves the release pr unable to reach a
green ci-ok — and nothing goes red.
EOF
    exit 1
fi

echo "release handoff is wired: ok"
