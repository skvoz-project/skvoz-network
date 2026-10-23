#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
revision=${SOURCE_REVISION:-${GITHUB_SHA:-$(git rev-parse HEAD)}}
docker buildx build --platform linux/amd64 --file docker/rust/Dockerfile --target "${TARGET:-artifacts}" \
  --build-arg SOURCE_REVISION="$revision" \
  --output "type=local,dest=${1:-temp/network-release}" .
