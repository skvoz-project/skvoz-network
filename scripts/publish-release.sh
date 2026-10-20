#!/bin/sh
# Publish once for the owning main commit; existing tags/releases fail closed.
set -eu
[ "$#" = 1 ] || { echo 'Usage: publish-release.sh ASSET_DIRECTORY' >&2; exit 2; }
: "${GITHUB_SHA:?owning source commit is required}"
: "${GITHUB_REPOSITORY:?owning repository is required}"
[ "$GITHUB_REPOSITORY" = skvoz-project/skvoz-network ]
[ "${GITHUB_REF:-}" = refs/heads/main ]
case "${GITHUB_EVENT_NAME:-}" in
  push|workflow_dispatch) ;;
  *) echo 'Publication requires a push or workflow_dispatch event' >&2; exit 1 ;;
esac
printf '%s\n' "$GITHUB_SHA" | LC_ALL=C grep -Eq '^[0-9a-f]{40}$'
tag=network-v0.7.0-$GITHUB_SHA
cd "$1"
sha256sum --check --strict SHA256SUMS
# Creating the ref atomically rejects a pre-existing or concurrently created tag.
gh api --method POST "repos/$GITHUB_REPOSITORY/git/refs" \
  -f "ref=refs/tags/$tag" -f "sha=$GITHUB_SHA" >/dev/null
gh release create "$tag" --repo "$GITHUB_REPOSITORY" --verify-tag \
  --title "Network 0.7.0 ($GITHUB_SHA)" --notes 'Linux runtime/helper и исходные SDK; SHA256SUMS фиксирует каждый архив.' \
  --latest=false SHA256SUMS \
  skvoz-network-0.7.0-x86_64-unknown-linux-gnu.tar.gz \
  skvoz-network-sdk-0.7.0-desktop-host.tar.gz \
  skvoz-network-sdk-0.7.0-portable-supplier.tar.gz
