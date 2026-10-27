#!/usr/bin/env bash
# Builds gpui-agent against crates.io's gpui-pre 0.3.9, the GPUI gpui-kit pins, with
# patches/gpui-pre-0.3.9 applied. It builds in a scratch workspace whose [patch.crates-io] points
# at the patched crates, the way an app on gpui-kit would consume gpui-agent, so this repository's
# manifests and Cargo.lock stay as they are. Prints the folder holding gpui-agent and the test app.
# Usage: scripts/gpui-pre.sh <work dir>
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
work=$(mkdir -p "${1:?give a work folder}" && cd "$1" && pwd)
vendor="$work/vendor"
ws="$work/ws"

# name version sha256 of crates.io's copies; gpui-pre-apple is unpatched and comes from crates.io.
crates='gpui-pre 0.3.9 34f61c1cfd36fc9c436af09308c963822ae27a45a84feb21a77ba5c9e051f1c0
gpui-pre-wgpu 0.3.9 5b1cdb26c1503bf9662ee8fbc5ab0b44b2deeee4b5c12dc67491ba6fb5fc544b
gpui-pre-linux 0.3.9 e28213c3fde6f13e91ba0b74b90cbaadec9373e36a031eda98561d31059870f3'

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

mkdir -p "$vendor/.crates"
patch_table='[patch.crates-io]'
while read -r name version sum; do
  file="$vendor/.crates/$name-$version.crate"
  [ -f "$file" ] || curl -fsSL "https://static.crates.io/crates/$name/$name-$version.crate" -o "$file"
  if [ "$(sha256 "$file")" != "$sum" ]; then
    echo "refused: $name-$version.crate does not match its checksum $sum" >&2
    exit 1
  fi
  rm -rf "${vendor:?}/$name-$version"
  tar -xzf "$file" -C "$vendor"
  # Inside another repository, git apply resolves paths from its root and skips them silently.
  (cd "$vendor/$name-$version" && GIT_CEILING_DIRECTORIES="$vendor" git apply \
    "$repo/patches/gpui-pre-0.3.9/$name-$version.patch")
  patch_table+=$'\n'"$name = { path = \"$vendor/$name-$version\" }"
done <<<"$crates"

mkdir -p "$ws/crates"
rsync -a --delete --exclude target "$repo/gpui/" "$ws/gpui/"
rsync -a --delete --exclude target "$repo/crates/protocol/" "$ws/crates/protocol/"
cp "$repo/rust-toolchain.toml" "$ws/"
# A gpui-pre consumer has no Zed checkout, so the scratch copies drop the `zed` choice.
zed_deps='^(gpui|gpui_linux|gpui_platform) = '
grep -vE "$zed_deps" "$repo/gpui/Cargo.toml" >"$ws/gpui/Cargo.toml"
printf '\n%s\n' "$patch_table" >>"$ws/gpui/Cargo.toml"
for manifest in "$ws/gpui/gpui-agent/Cargo.toml" "$ws/gpui/testapp/Cargo.toml"; do
  grep -vE "$zed_deps" "$manifest" | sed -e 's/^default = \["zed"\]/default = []/' -e 's/^zed = .*/zed = []/' >"$manifest.new"
  mv "$manifest.new" "$manifest"
done

(cd "$ws/gpui" && cargo build -q -p gpui-agent-testapp \
  --no-default-features --features gpui-agent-testapp/gpui-pre-0-3-9 >&2)
cargo build -q --manifest-path "$repo/Cargo.toml" -p gpui-agent-cli --target-dir "$ws/gpui/target" >&2
echo "$ws/gpui/target/debug"
