#!/bin/sh
# Copy source packages and metadata only; compilation is a separate supplier step.
set -eu
umask 022
if [ "$#" -ne 2 ]; then
  echo 'Usage: export-source-sdk.sh desktop-host|portable-supplier OUTPUT_DIRECTORY' >&2
  exit 2
fi
profile=$1
output=$2
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
[ ! -e "$output" ] || { echo 'output directory already exists' >&2; exit 1; }
mkdir -p -- "$output"
output=$(CDPATH= cd -- "$output" && pwd)
staging=$(mktemp -d)
trap 'rm -rf -- "$staging"' EXIT HUP INT TERM
sdk=$staging/sdk
mkdir "$sdk"
case "$profile" in
  desktop-host)
    for package in contracts client native; do
      mkdir "$sdk/$package"
      cp -R "$root/$package/src" "$sdk/$package/"
      for extra in tests README.md; do
        [ ! -e "$root/$package/$extra" ] || cp -R "$root/$package/$extra" "$sdk/$package/"
      done
      sed -e 's/^edition.workspace = true$/edition = "2024"/' \
          -e 's/^rust-version.workspace = true$/rust-version = "1.92"/' \
          -e 's/^license.workspace = true$/license = "MIT"/' \
          -e 's/^\[lints\]$/[lints.rust]/' \
          -e 's/^workspace = true$/unsafe_code = "forbid"/' \
          "$root/$package/Cargo.toml" > "$sdk/$package/Cargo.toml"
    done
    mkdir -p "$sdk/vendor/skvoz-core"
    cp -R "$root/vendor/skvoz-core/contracts" "$sdk/vendor/skvoz-core/"
    cp "$root/vendor/skvoz-core/LICENSE" "$sdk/vendor/skvoz-core/"
    cp "$root/LICENSE" "$root/Cargo.lock" "$sdk/"
    # These owned package documents refer to canonical supplier documentation.
    for package in contracts native; do
      sed 's|](../docs/network-runtime.md)|](https://github.com/skvoz-project/skvoz-network/blob/main/docs/network-runtime.md)|g' \
        "$sdk/$package/README.md" > "$staging/readme"
      mv "$staging/readme" "$sdk/$package/README.md"
    done
    cat > "$sdk/README.md" <<'README'
# Network Desktop SDK

Исходный SDK [skvoz-network](https://github.com/skvoz-project/skvoz-network)
0.7.0 содержит contracts/client/native и чистые contracts поставщика
[skvoz-core](https://github.com/skvoz-project/skvoz-core) 5.0.0.
Версии, профиль и зависимости закреплены в `sdk.json`, содержимое — в
`source-inputs.sha256`, supplier graph — в `Cargo.lock`.

Engine, NATS, Core binding и TLS adapter в этот профиль не включены.
Сборка продукта использует готовые runtime/helper outputs по
[контракту артефактов](https://github.com/skvoz-project/skvoz-network/blob/main/docs/artifacts.md).
Лицензия Network — [MIT](LICENSE); Core contracts сохраняют свою
[MIT лицензию](vendor/skvoz-core/LICENSE) и авторство поставщика.
README
    ;;
  portable-supplier)
    git -C "$root" ls-files --cached --others --exclude-standard -z > "$staging/inputs"
    tar -C "$root" --null --no-recursion -T "$staging/inputs" -cf "$staging/source.tar"
    tar -C "$sdk" -xf "$staging/source.tar"
    ;;
  *) echo 'unsupported source SDK profile' >&2; exit 2 ;;
esac
(cd "$sdk" && find . -type f -printf '%P\n' | LC_ALL=C sort) > "$staging/files"
while IFS= read -r path; do
  digest=$(sha256sum -- "$sdk/$path" | cut -d ' ' -f 1)
  printf '%s\t%s\n' "$digest" "$path"
done < "$staging/files" > "$sdk/source-inputs.sha256"
tree=$(sha256sum "$sdk/source-inputs.sha256" | cut -d ' ' -f 1)
core=$(sha256sum "$root/vendor/skvoz-core/sdk.sha256" | cut -d ' ' -f 1)
revision=null
if head=$(git -C "$root" rev-parse --verify HEAD 2>/dev/null) && state=$(git -C "$root" status --porcelain) && [ -z "$state" ]; then
  revision="\"$head\""
fi
if [ -n "${GITHUB_SHA:-}" ] && [ "$revision" != "\"$GITHUB_SHA\"" ]; then
  echo 'CI source must be the clean owning commit' >&2
  exit 1
fi
cat > "$sdk/sdk.json" <<JSON
{"schema":1,"component":"skvoz-network-sdk","version":"0.7.0","kind":"source-sdk","profile":"$profile","tuple":{"api":1,"network":6,"core":"5.0.0","helper":"0.6.0","core_abi":1,"network_abi":1},"source":{"repository":"https://github.com/skvoz-project/skvoz-network","revision":$revision,"tree_sha256":"$tree"},"dependencies":[{"component":"skvoz-core-source-sdk","version":"5.0.0","manifest_sha256":"$core"}],"qualification":"source candidate; compilation and platform checks pending"}
JSON
(cd "$sdk" && find . -type f -printf '%P\n' | LC_ALL=C sort) > "$staging/files"
archive=$output/skvoz-network-sdk-0.7.0-$profile.tar.gz
tar -C "$sdk" --no-recursion --verbatim-files-from -T "$staging/files" --sort=name --mtime='UTC 2026-10-19' --owner=0 --group=0 --numeric-owner --mode='u+rw,go+r,go-w,a+X' -czf "$archive"
cp "$sdk/sdk.json" "$output/sdk.json"
sha256sum -- "$archive" > "$output/archive.sha256"
cat "$output/archive.sha256"
