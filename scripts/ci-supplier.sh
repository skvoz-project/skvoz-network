#!/bin/sh
# Explicit Linux supplier; Core is fetched as an immutable release, never rebuilt.
set -eu
: "${GITHUB_SHA:?owning source commit is required}"
: "${NETWORK_OUTPUT:?set a fresh absolute output directory}"
root=$(pwd)
mkdir "$NETWORK_OUTPUT"
chmod 0777 "$NETWORK_OUTPUT"
image=rust:1.92.0-bookworm@sha256:e90e846de4124376164ddfbaab4b0774c7bdeef5e738866295e5a90a34a307a2
name=skvoz-network-ci-$$
cleanup() {
  code=$?
  trap - EXIT
  timeout --kill-after=2s 20s docker rm -f "$name" > /dev/null 2>&1 || true
  remaining=$(timeout --kill-after=2s 5s docker ps -aq --filter "name=^/$name$") || code=1
  [ -z "${remaining-unknown}" ] || code=1
  exit "$code"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
container() {
  timeout --signal=TERM --kill-after=10s 1200s docker run --name "$name" \
    --ulimit core=0 --cap-drop=ALL --security-opt=no-new-privileges:true \
    --mount "type=bind,src=$root,dst=/src,readonly" \
    --mount "type=bind,src=$NETWORK_OUTPUT,dst=/out" \
    --env CARGO_HOME=/out/cargo --env CARGO_TARGET_DIR=/out/target \
    --env CARGO_INCREMENTAL=0 --env GITHUB_SHA \
    --env GIT_CONFIG_COUNT=1 --env GIT_CONFIG_KEY_0=safe.directory --env GIT_CONFIG_VALUE_0=/src \
    --workdir /src "$image" sh -eu -c "$1"
  timeout --kill-after=2s 20s docker rm "$name" >/dev/null
}
container '
  cargo test --release --locked -p skvoz-network-artifact
  cargo build --release --locked -p skvoz-network-artifact
'
# The reviewed pin fixes all archive bytes; build.rs/binding check the Core interface.
pin=dependencies/core/x86_64-unknown-linux-gnu.json
url=$(jq -er '.artifact.url' "$pin")
sha256=$(jq -er '.artifact.sha256' "$pin")
curl --fail --location --proto '=https' --proto-redir '=https' --max-time 120 \
  --max-filesize 268435456 "$url" --output "$NETWORK_OUTPUT/core.tar.gz"
printf '%s  %s\n' "$sha256" "$NETWORK_OUTPUT/core.tar.gz" | sha256sum --check --strict -
mkdir -m 755 "$NETWORK_OUTPUT/core" "$NETWORK_OUTPUT/core/lib"
tar -xzf "$NETWORK_OUTPUT/core.tar.gz" -C "$NETWORK_OUTPUT/core" --no-same-owner \
  --no-same-permissions -- ./artifact.json ./lib/libskvoz_core.so
chmod 644 "$NETWORK_OUTPUT/core/artifact.json" "$NETWORK_OUTPUT/core/lib/libskvoz_core.so"
container '
  export SKVOZ_CORE_ARTIFACT=/out/core LD_LIBRARY_PATH=/out/core/lib
  cargo build --release --locked -p skvoz-network --features linux-runtime
  cargo build --release --locked -p skvoz-network-helper
  mkdir /out/release
  scripts/bundle-native.sh /out/target/release/skvoz-network-artifact \
    /out/target/release /out/core/lib/libskvoz_core.so \
    /out/release/skvoz-network-0.7.0-x86_64-unknown-linux-gnu.tar.gz
  for profile in desktop-host portable-supplier; do
    scripts/export-source-sdk.sh "$profile" "/out/$profile"
    cp "/out/$profile/skvoz-network-sdk-0.7.0-$profile.tar.gz" /out/release/
  done
  cd /out/release
  sha256sum ./*.tar.gz | sed "s|  ./|  |" > SHA256SUMS
  sha256sum --check --strict SHA256SUMS
'
