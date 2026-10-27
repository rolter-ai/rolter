#!/usr/bin/env bash
# builds the cyclonedx sbom that release.yml attests to one platform image (#1080)
#
#   scripts/image-sbom.sh <image@sha256:platform-digest> <lockfile-dir> <output.cdx.json>
#
# the sbom is two scans merged into one document:
#
#   - the image, by platform digest: the os packages and the files they own,
#     and the rust crates, which syft reads from the `cargo auditable` section
#     of the three rolter binaries (docker/Dockerfile builds with it). that is
#     the crate graph the binaries really link, with no dev-dependencies and no
#     unused features, which is why Cargo.lock is deliberately not scanned
#   - <lockfile-dir>: the tagged commit's ui/bun.lock, because the dashboard is
#     a vite bundle with no node_modules left to catalogue. any cargo package
#     that scan reports (release.yml still checks Cargo.lock out) is dropped
#
# the image scan must therefore contain cargo packages: none means a binary was
# built without `cargo auditable`, and attesting that sbom would claim an image
# with no rust in it, so the script fails instead
#
# the bun.lock scan still over-reports on purpose: syft 1.54's split of bun
# packages into production and dev-only keys its graph by package name, so when
# two versions of one package are locked the answer depends on map order and
# the production set changes from run to run (85, 96 and 98 packages in three
# runs over the same file). an sbom that silently drops shipped packages on
# some releases is worse than one that also lists the build tooling, and for a
# vulnerability scan over-reporting is the safe direction. drop
# SYFT_JAVASCRIPT_INCLUDE_DEV_DEPENDENCIES once syft fixes
# findDevOnlyBunPkgs (anchore/syft, tracked in #2747)
#
# the merged document keeps the image scan's metadata (the container is the
# subject) and appends the bun.lock components and dependency edges, with
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
if [ ! -f "$lockdir/ui/bun.lock" ]; then
  echo "error: $lockdir/ui/bun.lock is missing; the sbom would silently lose the dashboard's packages" >&2
  exit 1
fi

export SYFT_CHECK_FOR_APP_UPDATE=false
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# `registry:` so syft reads the pushed manifest, never a same-named image that
# happens to be in the local docker daemon
syft scan "registry:$image" -q -o "cyclonedx-json=$work/image.cdx.json"

# the rust packages come from the binaries, so the image scan has to have
# found them
rust="$(jq '[.components[]? | select((.purl // "") | startswith("pkg:cargo/"))] | length' "$work/image.cdx.json")"
if [ "$rust" -eq 0 ]; then
  echo "error: no cargo packages in the image scan of $image; the binaries were not built with cargo auditable, so refusing to attest an sbom without the rust crates" >&2
  exit 1
fi
echo "image binaries: $rust cargo packages"

# scanned from inside the directory so the lockfile components carry relative
# paths rather than the runner's checkout path
(cd "$lockdir" && SYFT_JAVASCRIPT_INCLUDE_DEV_DEPENDENCIES=true syft scan dir:. -q --source-name rolter-lockfiles -o "cyclonedx-json=$work/lock.cdx.json")

# cargo packages are the image scan's job; drop any the directory scan found in
# a Cargo.lock, with the dependency edges that name them
jq '
  ([.components[]? | select((.purl // "") | startswith("pkg:cargo/")) | ."bom-ref"]) as $cargo
  | .components |= [.[]? | select((.purl // "") | startswith("pkg:cargo/") | not)]
  | .dependencies |= [.[]? | select(.ref as $r | $cargo | index($r) | not)
      | .dependsOn |= [(. // [])[] | select(. as $d | $cargo | index($d) | not)]]
' "$work/lock.cdx.json" >"$work/npm.cdx.json"
n="$(jq '[.components[]? | select((.purl // "") | startswith("pkg:npm/"))] | length' "$work/npm.cdx.json")"
if [ "$n" -eq 0 ]; then
  echo "error: no npm packages found in $lockdir; refusing to attest an sbom without them" >&2
  exit 1
fi
echo "lockfile: $n npm packages"

jq -s '
  .[0] as $img | .[1] as $lock
  | $img
  | .components = ([($img.components // [])[], ($lock.components // [])[]] | unique_by(."bom-ref"))
  | .dependencies = ([($img.dependencies // [])[], ($lock.dependencies // [])[]] | unique_by(.ref))
  | .metadata.properties = ((.metadata.properties // []) + [
      {name: "rolter:sbom:sources", value: "image layers incl. cargo auditable binaries (syft registry scan) + ui/bun.lock of the tagged commit"}
    ])
' "$work/image.cdx.json" "$work/npm.cdx.json" >"$out"

echo "wrote $out: $(jq '.components | length' "$out") components"
