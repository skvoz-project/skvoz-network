#!/bin/sh
# Package only already built outputs. Compilation is an explicit supplier step.
set -eu
if [ "$#" -ne 4 ]; then
  echo 'Usage: bundle-native.sh ARTIFACT_TOOL BUILT_DIR CORE_LIBRARY OUTPUT_ARCHIVE' >&2
  exit 2
fi
artifact_tool=$1
built_dir=$2
core_library=$3
output_archive=$4
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
[ ! -e "$output_archive" ] || { echo 'output archive already exists' >&2; exit 1; }
staging=$(mktemp -d)
trap 'rm -rf -- "$staging"' EXIT HUP INT TERM
"$artifact_tool" "$root" "$built_dir" "$core_library" "$staging/bundle"
(cd "$staging/bundle" && find . -type f -printf '%P\n' | LC_ALL=C sort) > "$staging/files"
tar -C "$staging/bundle" --no-recursion --verbatim-files-from -T "$staging/files" --sort=name --mtime='UTC 2026-10-19' --owner=0 --group=0 --numeric-owner -czf "$output_archive"
sha256sum -- "$output_archive"
