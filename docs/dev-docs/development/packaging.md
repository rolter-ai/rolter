# Packaging & distribution

rolter ships three ways.

The unified `rolter` binary dispatches to both planes via subcommands:

```bash
rolter gateway --config rolter.toml     # data plane
rolter control --database-url postgres://…   # control plane + UI host
```

The standalone `rolter-gateway` / `rolter-control` binaries remain available.

## cargo

```bash
cargo install rolter            # unified launcher (from crates.io)
# or from source:
cargo install --path crates/rolter
```

## uv (PyPI wheel via maturin)

The wheel bundles the compiled `rolter` launcher so Python users can install the CLI with `uv`. `pyproject.toml` uses the maturin backend (`bindings = "bin"`, `manifest-path = crates/rolter/Cargo.toml`).

Each release publishes five wheels plus a source distribution:

| artifact | built on |
|---|---|
| `manylinux…x86_64` | `ubuntu-latest`, `target: x86_64` |
| `manylinux…aarch64` | `ubuntu-latest`, `target: aarch64` |
| `macosx…arm64` | `macos-latest` (Apple Silicon, native) |
| `macosx…x86_64` | `macos-latest`, cross-compiled `target: x86_64-apple-darwin` |
| `win_amd64` | `windows-latest` |
| `.tar.gz` (sdist) | `ubuntu-latest`, `command: sdist` |

The macOS x86_64 wheel is cross-compiled rather than built on an Intel runner —
the macOS SDK carries both architectures, so it needs no extra runner. The sdist
is the fallback for anything with no matching wheel: without one, `pip install
rolter` fails outright on an unlisted platform instead of building from source.
`verify-parity` asserts all six are present for the version, so a silently
missing platform fails the release rather than reaching a user.

```bash
uv tool install maturin       # one-time
uvx maturin build --release   # build a wheel into target/wheels/
uv tool install rolter        # once published to PyPI
```

## Docker

Multi-stage `docker/Dockerfile` builds the Rust binaries and the Bun-built UI, then assembles a slim runtime:

```bash
docker build -f docker/Dockerfile -t rolter:dev .
docker compose -f docker/docker-compose.yml up -d          # full stack with postgres/redis/clickhouse
```

## The version line

The workspace is on **`0.1.0`** (`Cargo.toml`, `[workspace.package]`), and every
crate inherits it. The line matters because release-plz derives the next version
from Cargo's SemVer compatibility rules rather than from the commit type, and
those rules change meaning below `1.0.0`:

| Current version | `fix:` | `feat:` | breaking (`!`) |
|---|---|---|---|
| `>= 1.0.0` | patch | minor | major |
| `0.x.y` (x >= 1) — **today** | patch | patch | minor |
| `0.0.z` | patch | patch | patch |

rolter sat on `0.0.z` until #501, where *every* commit type collapsed to a patch
bump: a release could never express that a feature or a breaking change had
landed. `0.1.0` restores that signal for breaking changes while deliberately
withholding the stable-API promise `1.0.0` carries — a `feat` is still a patch
until the 1.0.0 milestone closes and the version line moves again.

Moving the line is a one-time manual edit of `[workspace.package] version` plus
the matching `version = "…"` on each internal `[workspace.dependencies]` entry
(they are path+version deps so the published crates are not wildcards).
release-plz picks the new line up on the next Release PR and auto-bumps from
there.

### The helm chart's appVersion

`charts/rolter/Chart.yaml` carries two numbers and they mean different things.
`version:` is the *chart's* version, on its own cadence, and nothing here
touches it. `appVersion:` is which rolter a chart release deploys — the value
`helm list` prints and most dashboards surface — so it must equal the workspace
version.

Nothing kept it there. It sat at `0.0.9` while the workspace shipped `0.0.10`,
`0.0.11` and then `0.1.0`, pointing operators at a version that had never been
deployed (#1140). Two things now hold it:

- the `release-plz pr` job runs `scripts/sync-chart-appversion.py --fix` on the
  release branch and pushes the result, so the Release PR is already consistent
- `quality.yml`'s `helm chart` job runs the same script in check mode, so a
  disagreement fails CI rather than shipping — including if the release-branch
  step ever stops working

The script is also a `prek` hook on `Cargo.toml` and `Chart.yaml`, so a manual
version-line move is caught before it is pushed. To reconcile by hand:

```bash
python3 scripts/sync-chart-appversion.py --fix
```

## Release pipeline

Releases are fully automated from Conventional Commits. Two workflows do the
release work and `ci.yml` gates the release PR; the handoffs between them are
the part worth understanding, because all three are `workflow_dispatch` calls
made to get around GitHub's event suppression rather than ordinary triggers.

```text
merge to master
      │
      ▼
release-plz.yml ── release-pr ──►  "Release PR" (version bump + changelogs)
      │                                    │
      │                                    │ workflow_dispatch --ref <release branch>
      │                                    ▼          ← dispatch-release-pr-ci
      │                                 ci.yml ──►  ci-ok on the Release PR
      │
      │ (that PR is merged)
      ▼
release-plz.yml ── verify ──► release ──►  crates.io publish
                                           tag v{version}
                                           github release
      │
      │ workflow_dispatch -f tag=v{version}      ← dispatch-artifact-release
      ▼
release.yml
  │
  ├─ gate ──── verify-external-checks (ci-ok + CodeQL green for the tagged sha)
  │
  ├─ build ─── build-wheels  (5 wheels + sdist)
  │            build-image   (per arch, pushed as untagged digests)
  │
  ├─ smoke ─── smoke-wheels  (install each wheel, run `rolter --version`)
  │            smoke-image   (run each image digest, check its version)
  │
  ├─ publish ─ publish-pypi    (trusted publishing, OIDC)
  │            publish-docker  (assemble tag manifests: GHCR + Docker Hub)
  │
  └─ check ─── verify-parity  (all channels serve {version})
```

The stages are a barrier, not decoration. Every publish job depends on *every*
build and smoke job, so a release is all-or-nothing: a failed wheel can no
longer leave container images published against a version that has nothing on
PyPI. Before this split, `publish-docker` did not depend on `build-wheels` at
all, and exactly that partial release was possible.

Two properties make the barrier real:

- **Images are built as untagged digests** (`push-by-digest`). A digest nobody
  can resolve by tag is not a release; `publish-docker` only assembles the
  `:{version}` and `:latest` manifests once everything else has passed, from
  those same digests — so the image is never rebuilt.
- **Nothing is published untested.** The smoke stage installs each wheel and
  runs each image digest, asserting `rolter --version` matches the tag, while
  the artifacts are still private. The wheel install uses `--no-index`, so it
  can only resolve from the freshly built `dist/` and can never pass by
  silently pulling an older rolter from PyPI.

Each build is its own job, so a single flaky platform can be re-run on its own
without re-publishing anything that already succeeded.

### Why the release PR needs a dispatch too

The same suppression hides the release PR itself. `ci-ok` is the only required
status check on master, and only `ci.yml` reports it — but release-plz pushes
the release branch and opens its PR with the repository `GITHUB_TOKEN`, so
neither `push` nor `pull_request` fires and the required context is never
reported. The PR stays at `BLOCKED` no matter how long you wait, and the only
way to merge it is an admin bypass. Every release from the introduction of
`ci-ok` up to [#1025] went out that way; the only checks that ever landed on a
release PR were the externally-sourced ones (the CodeQL app, GitGuardian),
because those do not come from a workflow event.

`dispatch-release-pr-ci` closes it with the same tool as the tag handoff. It
runs after `release-plz-pr` — so it sees the head that the `sync chart
appVersion` step leaves behind, not the one before it — finds the open PR whose
head branch starts with `release-plz-`, and runs `gh workflow run ci.yml --ref
<branch>`. A `workflow_dispatch` run's check-runs attach to the head commit of
the ref, which is exactly the commit branch protection is looking at. Like the
tag handoff, the job fails if the dispatch produces no run.

It deliberately does **not** read the branch from the action's `prs` output:
that output is populated only on the run that *creates* the PR, while the branch
is force-pushed on every later master commit and needs re-gating each time.

One check is genuinely absent on a dispatched run: `pr-title` is
`if: github.event_name == 'pull_request'`, so it is skipped, and `ci-ok`
tolerates a skipped `pr-title` the same way it does on a push build. That is
acceptable here because release-plz writes the release PR title itself and it is
already a valid Conventional Commit line.

### Why the explicit dispatch

`release.yml` also has a `push: tags` trigger, but it never fires for a real
release. release-plz creates the tag with the repository `GITHUB_TOKEN`, and
[GitHub suppresses downstream workflow events for token-created refs][gh-token]
to prevent recursive runs. `workflow_dispatch` is the documented exception — it
always creates a run, even from the `GITHUB_TOKEN` — so `release-plz.yml` ends
with a `dispatch-artifact-release` job that calls
`gh workflow run release.yml -f tag=vX.Y.Z`. That job holds `actions: write` and
nothing else, and it fails if the dispatch produces no run.

Without it the pipeline half-works in the worst way: the GitHub release and
crates.io advance while no wheel is ever built, and every job stays green. That
is how v0.0.6 through v0.0.10 shipped while PyPI sat on 0.0.5 ([#903]).
`scripts/check-release-handoff.sh` (a merge gate in `quality.yml` and a prek
hook) asserts the wiring is still in place.

### Which tag the dispatch carries

`releases`, the output release-plz hands back, contains a `tag` for **every**
published crate — not only the one with `git_tag_enable`. The crates.io-only
members get a derived `<crate>-v<version>` string that was never pushed to git.
Only `rolter-gateway` is configured with `git_tag_name = "v{{ version }}"`, so
`resolve release tag` selects that entry **by package name**; an empty result
means nothing was released on this push and the dispatch job is skipped.

Taking the first tagged entry instead is what broke v0.0.11 ([#1026]): the array
starts with `rolter-core`, the dispatch carried `rolter-core-v0.0.11`, and
`release.yml` failed at checkout on a ref that does not exist — so v0.0.11 went
to crates.io and GitHub Releases with no wheel, and PyPI stayed on 0.0.10. The
step now also rejects any resolved tag that is not `vX.Y.Z`, so a bad ref fails
before it is dispatched rather than halfway through the artifact build.

[gh-token]: https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow#triggering-a-workflow-from-a-workflow
[#903]: https://github.com/rolter-ai/rolter/issues/903
[#1025]: https://github.com/rolter-ai/rolter/issues/1025
[#1026]: https://github.com/rolter-ai/rolter/issues/1026

### Parity gate

`verify-parity` runs with `always()` at the end of `release.yml` and asserts
that every enabled channel actually serves the tagged version: the GitHub
release exists, crates.io has `rolter {version}`, PyPI has it (when
`PYPI_PUBLISH_ENABLED` is `true`), and GHCR has the manifest (when
`DOCKER_PUBLISH_ENABLED` is `true`). A skipped or failed publish turns the run
red instead of quietly leaving a channel behind.

### Publishing gates

| Gate | Effect |
|---|---|
| `verify-external-checks` | `ci-ok` **and** CodeQL recorded success for the tagged commit; fail-closed |
| `RELEASE_REQUIRED_CHECKS` repo variable | exact check-run names the gate above requires (comma-separated) |
| `PYPI_PUBLISH_ENABLED` repo variable | must be `"true"` or the PyPI publish is skipped |
| `DOCKER_PUBLISH_ENABLED` repo variable | must be `"true"` or the image publish is skipped |
| `pypi` environment | PyPI trusted publishing via OIDC; no long-lived token is stored |

Wheels are built with `maturin-action` but uploaded with `pypa/gh-action-pypi-publish`:
`maturin upload` is deprecated and slated for removal ([PyO3/maturin#2334]). The
publisher identity PyPI matches on is the repository, workflow filename and
environment — not the tool — so the swap is transparent to the trusted-publisher
config, and it adds PEP 740 attestations on the `id-token` grant the job already
holds.

[PyO3/maturin#2334]: https://github.com/PyO3/maturin/issues/2334

### The gate is asserted, not re-run

`release.yml` does **not** run `quality.yml` itself. It asserts that the tagged
commit already passed it, by requiring `ci-ok` among the check-runs recorded for
that SHA. That is deliberate, and it is what makes the gate correct:

A local reusable workflow (`uses: ./…`) always checks out the *caller's* ref. On
a `workflow_dispatch` the caller ref is `master`, while `build-wheels` checks out
`inputs.tag` — so a re-run verified master and shipped the tag ([#988]). It
passed, and told you nothing about what was being packaged. Threading the tag
into `quality.yml` fixes that but makes the shared workflow check out an
arbitrary dispatch-supplied ref in a default-branch context, whose caches
trusted runs later restore — cache poisoning, and CodeQL flags it.

Asserting settles both. Every commit on master carries a `ci-ok` check-run from
`ci.yml`, and release-plz re-runs the gate on the release commit before tagging,
so a tagged commit is verified by construction. The assertion binds to the
*tagged* SHA — which re-running never did — costs no duplicate 20-minute run,
and checks out nothing.

Because release-plz dispatches the moment it finishes tagging, `ci-ok` is often
still running for that commit. A pending check is therefore expected, not a
failure: the job waits up to 45 minutes for a verdict, fails immediately on a
real non-success, and fails closed if a required check never appears.

[#988]: https://github.com/rolter-ai/rolter/issues/988

`RELEASE_REQUIRED_CHECKS` holds exact check-run *names*, so it rots whenever a
scanner is renamed or reconfigured — and since the gate is fail-closed, a stale
name silently blocks every release instead of failing at the source. This bit
rolter once already: the variable still named the CodeQL *default setup* jobs
(`Analyze (rust)`, …) after the repo moved to advanced setup (`codeql (rust)`,
…), so no release could publish even with a working tag dispatch. If the gate
reports "required check … not found", compare it against the check-run names
the job log prints and update the variable.

### Releasing a tag by hand

For a backfill, or if a dispatch was lost, run the artifact half yourself from
the default branch:

```bash
gh workflow run release.yml -f tag=v0.0.10
gh run watch "$(gh run list --workflow=release.yml --limit 1 --json databaseId -q '.[0].databaseId')"
```

The upload uses `--skip-existing`, so re-running a tag that partly published is
safe. Verify the result:

```bash
curl -s https://pypi.org/pypi/rolter/json | jq -r .info.version
uv tool install rolter && rolter --version
```

### Backfill policy

Only the **current** release is backfilled to PyPI. The versions the broken
handoff skipped (v0.0.6 – v0.0.9) stay unpublished: they are superseded pre-1.0
releases, `uv tool install rolter` / `pip install rolter` resolve to the latest
version regardless, and publishing them retroactively would put four versions on
the index that no user ever pinned, dated years after their tags. Anyone who
needs one of them can build from the tag or `cargo install rolter@0.0.x` from
crates.io, which has the complete series.
