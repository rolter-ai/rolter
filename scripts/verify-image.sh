#!/usr/bin/env bash
# verifies a published rolter container image: its signature, its sbom and its
# build provenance (#1080)
#
#   scripts/verify-image.sh ghcr.io/rolter-ai/rolter:1.0.0
#   scripts/verify-image.sh --skip-provenance ghcr.io/rolter-ai/rolter@sha256:<digest>
#
# this is the one verification command there is. release.yml's `sign-images`
# job runs it against every image it has just signed, and the user docs
# (docs/user-docs/deployment/verify-images.mdx) hand readers this same script,
# so the documented check cannot drift from the one ci proves passes.
#
# it checks, and exits non-zero on the first failure:
#
#   1. the manifest list the reference resolves to carries a keyless cosign
#      signature whose fulcio certificate names release.yml in this repository,
#      run from master or from a release tag, as the signer
#   2. each platform image in that list (linux/amd64 and linux/arm64) carries
#      the same signature, and a cyclonedx sbom attestation from the same signer
#   3. the manifest list has a slsa build provenance attestation from
#      release.yml in this repository, checked with `gh attestation verify`
#      (skip with --skip-provenance where the github cli is not available)
#
# needs: cosign 3 or later (the signatures are sigstore bundles stored as oci
# referrers, which cosign 2 does not look for by default), jq, and either
# crane or docker with buildx to read the manifest list. the provenance check
# also needs gh, signed in (`gh auth login`) or given a GH_TOKEN, because it
# reads github's attestation api. everything is fetched over the network
#
# environment (all optional; the defaults are what a real release is held to):
#   ROLTER_VERIFY_REPOSITORY  owner/repo whose release.yml must have signed
#                             (default rolter-ai/rolter)
#   ROLTER_VERIFY_PLATFORMS   space-separated platforms that must be present
#                             (default "linux/amd64 linux/arm64")
set -euo pipefail

usage() {
  echo "usage: $0 [--skip-provenance] <image reference>" >&2
  echo "  e.g. $0 ghcr.io/rolter-ai/rolter:1.0.0" >&2
}

skip_provenance=0
ref=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --skip-provenance) skip_provenance=1 ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      usage
      exit 2
      ;;
    *)
      if [ -n "$ref" ]; then
        usage
        exit 2
      fi
      ref="$1"
      ;;
  esac
  shift
done
if [ -z "$ref" ]; then
  usage
  exit 2
fi

repository="${ROLTER_VERIFY_REPOSITORY:-rolter-ai/rolter}"
platforms="${ROLTER_VERIFY_PLATFORMS:-linux/amd64 linux/arm64}"
workflow=".github/workflows/release.yml"
issuer="https://token.actions.githubusercontent.com"
# the san of a github actions fulcio certificate is the workflow file at the ref
# it ran on: refs/heads/master when release-plz dispatches it, refs/tags/v* when
# a tag push starts it. a run from any other branch is not a release
identity_re="^https://github\.com/${repository//./\\.}/\.github/workflows/release\.yml@refs/(heads/master|tags/v[0-9]+\.[0-9]+\.[0-9]+.*)$"

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: $1 is required: $2" >&2
    exit 1
  fi
}
need cosign "https://docs.sigstore.dev/cosign/system_config/installation/"
need jq "https://jqlang.org/download/"
if [ "$skip_provenance" = 0 ]; then
  need gh "https://cli.github.com (or pass --skip-provenance)"
fi

# the raw manifest, without letting a tool resolve a platform for us
raw_manifest() {
  if command -v crane >/dev/null 2>&1; then
    crane manifest "$1"
  elif command -v docker >/dev/null 2>&1 && docker buildx version >/dev/null 2>&1; then
    docker buildx imagetools inspect --raw "$1"
  else
    echo "error: crane or docker buildx is required to read the manifest list" >&2
    exit 1
  fi
}

# the repository part of the reference: drop @digest, then a :tag in the last
# path segment (a registry port lives in the first one, so it survives)
name="${ref%@*}"
last="${name##*/}"
if [ "$last" != "${last%:*}" ]; then
  name="${name%:*}"
fi

cosign_identity=(
  --certificate-identity-regexp "$identity_re"
  --certificate-oidc-issuer "$issuer"
  --certificate-github-workflow-repository "$repository"
)

echo "verifying $ref"
echo "  signer: $repository/$workflow (master or a v* tag), issuer $issuer"

cosign verify "${cosign_identity[@]}" "$ref" >/dev/null
echo "ok: manifest list signature"

manifest="$(raw_manifest "$ref")"
for platform in $platforms; do
  os="${platform%%/*}"
  arch="${platform#*/}"
  digest="$(jq -r --arg os "$os" --arg arch "$arch" \
    '[.manifests[]? | select(.platform.os == $os and .platform.architecture == $arch) | .digest] | if length == 1 then .[0] else empty end' \
    <<<"$manifest")"
  if [ -z "$digest" ]; then
    echo "error: $ref has no single $platform image in its manifest list" >&2
    exit 1
  fi
  image="$name@$digest"
  cosign verify "${cosign_identity[@]}" "$image" >/dev/null
  echo "ok: $platform signature ($digest)"
  cosign verify-attestation --type cyclonedx "${cosign_identity[@]}" "$image" >/dev/null
  echo "ok: $platform cyclonedx sbom attestation"
done

if [ "$skip_provenance" = 1 ]; then
  echo "skipped: build provenance (--skip-provenance)"
else
  gh attestation verify "oci://$ref" \
    --repo "$repository" \
    --signer-workflow "$repository/$workflow" >/dev/null
  echo "ok: slsa build provenance"
fi

echo "verified $ref"
