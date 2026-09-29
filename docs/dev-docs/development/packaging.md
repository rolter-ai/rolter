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

| artifact            | built on                                                     |
| ------------------- | ------------------------------------------------------------ |
| `manylinux…x86_64`  | `ubuntu-latest`, `target: x86_64`                            |
| `manylinux…aarch64` | `ubuntu-latest`, `target: aarch64`                           |
| `macosx…arm64`      | `macos-latest` (Apple Silicon, native)                       |
| `macosx…x86_64`     | `macos-latest`, cross-compiled `target: x86_64-apple-darwin` |
| `win_amd64`         | `windows-latest`                                             |
| `.tar.gz` (sdist)   | `ubuntu-latest`, `command: sdist`                            |

The macOS x86_64 wheel is cross-compiled rather than built on an Intel runner —
the macOS SDK carries both architectures, so it needs no extra runner. The sdist
is the fallback for anything with no matching wheel: without one, `pip install
rolter` fails outright on an unlisted platform instead of building from source.
`verify-parity` asserts all six are present for the version, so a silently
missing platform fails the release rather than reaching a user.

Those builds run only after crates.io has published and the tag is pushed, so a
packaging mistake found there leaves a release with no wheel. The
`package (publish verify)` job in `quality.yml` catches most of them on the PR:
it runs `maturin sdist` from the repo root, which parses `pyproject.toml` and
`[tool.maturin]`, resolves `manifest-path`, the bindings, the readme and the
license, and packages the workspace path dependencies. It takes seconds because
it compiles nothing, so a failure that appears only when maturin compiles a
wheel for a release target still shows up first in `build-wheels`.

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

| Current version              | `fix:` | `feat:` | breaking (`!`) |
| ---------------------------- | ------ | ------- | -------------- |
| `>= 1.0.0`                   | patch  | minor   | major          |
| `0.x.y` (x >= 1) — **today** | patch  | patch   | minor          |
| `0.0.z`                      | patch  | patch   | patch          |

rolter sat on `0.0.z` until #501, where _every_ commit type collapsed to a patch
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
`version:` is the _chart's_ version, on its own cadence, and nothing here
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

Releases are automated from Conventional Commits, apart from one step: a
maintainer labels the release PR `release:ready` when it should go out (see
[cutting a release](#cutting-a-release)). Two workflows do the release work and
`ci.yml` gates the release PR; the handoffs between them are the part worth
understanding, because each one is a `workflow_dispatch` call made to get
around GitHub's event suppression rather than an ordinary trigger.

```text
merge to master
      │
      ▼
release-plz.yml ── release-pr ──►  "Release PR" (version bump + changelogs)
      │                                    │
      │                                    │ a maintainer adds the release:ready label
      │                                    │
      │                                    │ workflow_dispatch --ref <release branch>
      │                                    ▼          ← release-pr-ready.yml, on the label
      │                                 ci.yml        ← dispatch-release-pr-ci, on each
      │                                    │            later push while it stays
      │                                    ▼
      │                          ci-ok on the Release PR
      │
      │ (that PR is merged)
      ▼
release-plz.yml ── release-gate ──► release ──►  crates.io publish
                   (ci-ok of this sha's          tag v{version}
                    ci.yml push run)             github release
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

The stages are a barrier, not decoration. Every publish job depends on _every_
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

### Cutting a release

release-plz keeps one release PR open, on a branch named
`release-plz-<timestamp>`, and rewrites it on every push to master. Nothing
runs `ci.yml` on it until a maintainer decides the release should go out:

1. Add the `release:ready` label to the release PR:

   ```bash
   gh pr edit <number> --add-label release:ready
   ```

   `release-pr-ready.yml` runs on that label event and dispatches `ci.yml` on
   the release branch. release-plz updates the same PR in place, so the label
   survives its force-pushes, and every later push to master that moves the
   branch dispatches `ci.yml` again through `dispatch-release-pr-ci` for as
   long as the label stays.

2. Wait for `ci-ok` on the release PR, then merge it. The merge commit's own
   push run then gates the crates.io publish
   ([below](#the-cratesio-publish-waits-for-the-push-run)).

3. To hold back a release that was labelled too early, remove the label. The
   head that already has `ci-ok` stays mergeable, but the next push that
   rewrites the branch gets no dispatch, so the PR goes back to `BLOCKED`.

If the label produced no `ci.yml` run, dispatch it by hand:

```bash
gh workflow run ci.yml --ref <release branch>
```

That covers a failed `release pr ready` run, and a release PR with a merge
conflict, on which GitHub starts no `pull_request` workflow at all. release-plz
rebuilds the branch from master on every push, so a conflict there is rare.

### The crates.io publish waits for the push run

`release-plz release` publishes to crates.io, pushes the tag and creates the
GitHub release with a persisted `contents: write` token, so it must never start
on a commit nothing verified (ROL-103). It needs the `release-gate` job and
runs only when that job's `verified` output is `true`. `release-gate` holds a
read-only token (`contents: read`, `actions: read`) and does two things:

1. `scripts/unpublished-crates.sh` lists every workspace crate whose `publish`
   is not `[]` in `cargo metadata` (today everything except `rolter-ui`) and
   looks its version up on crates.io. When every version is already there,
   which is every push except the merge of the release PR, the job prints a
   notice and stops without setting `verified`, so `release-plz release` is
   skipped. A lookup that fails for any reason counts as unpublished.
2. Otherwise `scripts/wait-for-ci-gate.sh` waits for the `ci.yml` **push** run
   on the same commit
   (`actions/workflows/ci.yml/runs?head_sha=<sha>&event=push&branch=master`)
   and requires that run's `ci-ok` job to have concluded `success`. It fails
   closed on an API error that survives three attempts, on any other
   conclusion, on a finished run with no `ci-ok` job, and after 90 minutes.
   Its last command writes `verified=true` to the step output, and the step
   runs that script and nothing else, so only a successful exit sets it.

The gate is bound to a run, not to a check-run name. Any workflow can post a
check-run called `ci-ok` on a master commit: a `pull_request` run from master
into another branch reports on master's head, and workflows fired by outsiders
write check-runs there too. A `ci.yml` push run on a sha can only come from
`ci.yml` at that sha, and a later push never cancels it. `release.yml`'s
`verify-external-checks` still matches check-run names
([below](#the-gate-is-asserted-not-re-run)); the two are separate mechanisms.

The publish is bound to the gate, not to the detector. A detector that wrongly
reports nothing pending leaves `verified` unset, which delays a release but
never publishes one unverified. `scripts/check-release-handoff.py` asserts that
binding, that the wait step is the bare script call, and that the script's one
write of `verified` is its last command. `scripts/test-release-gate.sh` runs
both scripts against a fake `gh`, `curl`, `cargo` and clock, checking that the
output is written exactly when the wait exits 0, as a step in `quality.yml`'s
`release handoff wired` job and as a prek hook.

The job waits rather than re-running `quality.yml` on the merge commit, which
cost 27 jobs on every push while `ci.yml`'s own push run was gating the same
sha. A push with nothing to publish now costs one short job. A release push
holds one runner, and the `release-plz` concurrency group, for as long as the
push gate takes; the 90-minute deadline leaves room above the slowest push gate
measured (61 minutes).

A timeout or a red `ci-ok` fails `release gate` and skips the publish. Get
`ci-ok` green on that push run first (re-run its failed jobs if the failure was
a flake), then re-run the gate and everything after it:

```bash
gh run rerun <release-plz run id> --failed
```

If a later push's release-plz run starts first, it finds the version still
unpublished, gates its own commit and publishes from that later tree, which is
verified the same way.

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
head branch starts with `release-plz-` and lives in this repository, and, if
that PR carries the `release:ready` label, runs `gh workflow run ci.yml --ref
<branch>`. A `workflow_dispatch` run's check-runs attach to the head commit of
the ref, which is exactly the commit branch protection is looking at. Like the
tag handoff, the job fails if the dispatch produces no run.

Before #2025 it dispatched on every push to master. Each of those runs gated a
PR nobody was about to merge, and the next push often cancelled it part-way:
11 of the 33 release-PR dispatches between 2026-09-21 and 09-28 were cancelled
mid-flight, and on 09-28 the dispatches took 16.7% of the day's runner time
([ADR-0034](../adr/2026-09-29-ci-runner-budget.md)). The label moves that cost
to the moment someone intends to merge. The check fails closed. No open release
PR, or one without the label (a listing that shows no labels at all included),
means no dispatch, and the PR stays `BLOCKED` until someone adds the label. An
API error fails the job.

A label added by a person is not a `GITHUB_TOKEN` event, so it does start a
workflow, and `release-pr-ready.yml` is the one it starts. It runs on
`pull_request` (`labeled`) and dispatches only for `release:ready` on a
`release-plz-` branch of this repository; every other label event skips its one
job without taking a runner. The job holds `actions: write`, checks nothing out
and uses no action. It uses `pull_request` rather than `pull_request_target`
because a fork's `pull_request` run gets a read-only token whatever the job
asks for, while `pull_request_target` would hand it a write token; zizmor's
`dangerous-triggers` audit rejects the latter. A branch of this repository can
only come from someone who can already push workflows, so running that
branch's copy of the workflow grants nothing new.

It deliberately does **not** read the branch from the action's `prs` output:
that output is populated only on the run that _creates_ the PR, while the branch
is force-pushed on every later master commit and needs re-gating each time.

One check is genuinely absent on a dispatched run: `ci-ok`'s `pr-title` step
runs only on `pull_request`, so it is skipped, and the verdict tolerates that
skip with a `::warning::` saying the title went unvalidated. That is acceptable
here because release-plz writes the release PR title itself and it is already a
valid Conventional Commit line.

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
`scripts/check-release-handoff.py` (a merge gate in `quality.yml` and a prek
hook) asserts the wiring is still in place, the crates.io publish gate above
included. It reads `release-plz.yml`, `release-pr-ready.yml`, `release.yml`
and `ci.yml` as parsed YAML (and `scripts/wait-for-ci-gate.sh` as shell text)
and checks structure: a job exists, its `needs` set holds the required job ids,
a trigger or dispatch input is declared. It also holds both release-PR
dispatches to the same label name, so renaming `release:ready` in one workflow
fails the check instead of leaving one path that never fires, and it keeps
`release-pr-ready.yml` free of any action, a checkout included.
The few assertions that are text by nature, a `gh workflow run` command or the
`push-by-digest=true` option, look only inside the job they belong to, and they
read its shell with the comments removed: a dispatch commented out to pause
releases fails the check rather than passing it. A job or step switched off
with `if: false` counts as missing. So the check holds however the files are
laid out, prettier's reflow included. The `gh api` run confirmations keep
`event=workflow_dispatch` in the URL path; any other query field is accepted
as a `-f` flag only when the call pins `-X GET`. Run it
with `uv run --script scripts/check-release-handoff.py`; `--self-test` breaks
each invariant in a copy of the workflows and fails unless the matching check
catches it, which is how a check edited into one that can never fail gets
noticed.

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

| Gate                                    | Effect                                                                                                    |
| --------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `release-gate` (`release-plz.yml`)      | crates.io publish, tag and GitHub release wait for `ci-ok` on the commit's `ci.yml` push run; fail-closed |
| `release:ready` label on the release PR | `ci.yml` runs on the release PR only while it is present, so without it the PR stays `BLOCKED`            |
| `verify-external-checks`                | `ci-ok` **and** CodeQL recorded success for the tagged commit; fail-closed                                |
| `RELEASE_REQUIRED_CHECKS` repo variable | exact check-run names `verify-external-checks` requires (comma-separated)                                 |
| `PYPI_PUBLISH_ENABLED` repo variable    | must be `"true"` or the PyPI publish is skipped                                                           |
| `DOCKER_PUBLISH_ENABLED` repo variable  | must be `"true"` or the image publish is skipped                                                          |
| `pypi` environment                      | PyPI trusted publishing via OIDC; no long-lived token is stored                                           |

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

A local reusable workflow (`uses: ./…`) always checks out the _caller's_ ref. On
a `workflow_dispatch` the caller ref is `master`, while `build-wheels` checks out
`inputs.tag` — so a re-run verified master and shipped the tag ([#988]). It
passed, and told you nothing about what was being packaged. Threading the tag
into `quality.yml` fixes that but makes the shared workflow check out an
arbitrary dispatch-supplied ref in a default-branch context, whose caches
trusted runs later restore — cache poisoning, and CodeQL flags it.

Asserting settles both. Every commit on master carries a `ci-ok` check-run from
`ci.yml`, and release-plz tags only after `release-gate` has seen `ci-ok` succeed
on the release commit's `ci.yml` push run
([above](#the-cratesio-publish-waits-for-the-push-run)), so a tagged commit is
verified by construction. The assertion binds to the
_tagged_ SHA — which re-running never did — costs no duplicate 20-minute run,
and checks out nothing.

On the release-plz path the loop normally passes on its first poll:
`release-gate` saw `ci-ok` finish before the tag existed, and `ci-ok` needs
every CodeQL leg. A tag dispatched by hand, or a check re-run after tagging, can
still leave a required check pending, and that is expected, not a failure: the
job waits up to 45 minutes for a verdict, fails immediately on a real
non-success, and fails closed if a required check never appears.

[#988]: https://github.com/rolter-ai/rolter/issues/988

`RELEASE_REQUIRED_CHECKS` holds exact check-run _names_, so it rots whenever a
scanner is renamed or reconfigured — and since the gate is fail-closed, a stale
name silently blocks every release instead of failing at the source. This bit
rolter once already: the variable still named the CodeQL _default setup_ jobs
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
