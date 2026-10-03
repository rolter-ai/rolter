#!/usr/bin/env bash
# builds the cyclonedx sbom that release.yml attests to one platform image (#1080)
#
#   scripts/image-sbom.sh <image@sha256:platform-digest> <lockfile-dir> <output.cdx.json>
#
# an sbom of the image alone misses most of what is in it. syft over the image
# layers finds the distroless debian packages, but the three rolter binaries are
# plain `cargo build` output with no `cargo auditable` section for syft to read,
# and the dashboard is a vite bundle with no node_modules left to catalogue. so
# the sbom is two scans merged into one document:
#
#   - the image, by platform digest: the os packages and the files they own
#   - <lockfile-dir>: the tagged commit's Cargo.lock and ui/bun.lock, and only
#     those (release.yml sparse-checks them out), so syft's lockfile catalogers
#     see the crate graph the binaries were built from and the dashboard's npm
#     packages
#
# both lockfiles over-report rather than under-report. Cargo.lock pins
# dev-dependencies and every optional or platform-specific crate too, and the
# bun.lock scan keeps dev dependencies on purpose: syft 1.54's split of bun
# packages into production and dev-only keys its graph by package name, so when
# two versions of one package are locked the answer depends on map order and
# the production set changes from run to run (85, 96 and 98 packages in three
# runs over the same file). an sbom that silently drops shipped packages on
# some releases is worse than one that also lists the build tooling, and for a
# vulnerability scan over-reporting is the safe direction
#
# the merged document keeps the image scan's metadata (the container is the
# subject) and appends the lockfile components and dependency edges, with
# bom-refs de-duplicated. needs syft and jq on PATH
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $0 <image@sha256:digest> <lockfile-dir> <output.cdx.json>" >&2
  exit 2
fi
image="$1"
lockdir="$2"
out="$3"

case "$image" in
  *@sha256:*) ;;
  *)
    echo "error: $image is not pinned by digest; the sbom must describe exactly one manifest" >&2
    exit 2
    ;;
esac
for f in Cargo.lock ui/bun.lock; do
  if [ ! -f "$lockdir/$f" ]; then
    echo "error: $lockdir/$f is missing; the sbom would silently lose that dependency tree" >&2
    exit 1
  fi
done

export SYFT_CHECK_FOR_APP_UPDATE=false
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# `registry:` so syft reads the pushed manifest, never a same-named image that
# happens to be in the local docker daemon
syft scan "registry:$image" -q -o "cyclonedx-json=$work/image.cdx.json"

# scanned from inside the directory so the lockfile components carry relative
# paths rather than the runner's checkout path
(cd "$lockdir" && SYFT_JAVASCRIPT_INCLUDE_DEV_DEPENDENCIES=true syft scan dir:. -q --source-name rolter-lockfiles -o "cyclonedx-json=$work/lock.cdx.json")

for kind in cargo npm; do
  n="$(jq --arg k "pkg:$kind/" '[.components[]? | select((.purl // "") | startswith($k))] | length' "$work/lock.cdx.json")"
  if [ "$n" -eq 0 ]; then
    echo "error: no $kind packages found in $lockdir; refusing to attest an sbom without them" >&2
    exit 1
  fi
  echo "lockfiles: $n $kind packages"
done

jq -s '
  .[0] as $img | .[1] as $lock
  | $img
  | .components = ([($img.components // [])[], ($lock.components // [])[]] | unique_by(."bom-ref"))
  | .dependencies = ([($img.dependencies // [])[], ($lock.dependencies // [])[]] | unique_by(.ref))
  | .metadata.properties = ((.metadata.properties // []) + [
      {name: "rolter:sbom:sources", value: "image layers (syft registry scan) + Cargo.lock + ui/bun.lock of the tagged commit"}
    ])
' "$work/image.cdx.json" "$work/lock.cdx.json" >"$out"

echo "wrote $out: $(jq '.components | length' "$out") components"
