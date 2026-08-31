#!/bin/sh
# Build the browser module, both ways it is wanted.
#
#   no-modules  the shipping shape: a classic <script> that defines
#               a global, which is what a page with a nonce'd
#               script tag and no bundler can actually load.
#   nodejs      the certification shape: what `certify/wasm-diff.sh`
#               can require, so the sweeps can drive it.
#
# Both come from the very same wasm, so what the sweeps certify is
# what the page ships.
#
# Needs the target and a wasm-bindgen CLI matching the crate's own
# version (`grep -A1 'name = "wasm-bindgen"' Cargo.lock`):
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <that version>

set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out=${1:-"$root/target/wasm"}

cargo build --release --target wasm32-unknown-unknown --manifest-path "$here/Cargo.toml"

built="$here/target/wasm32-unknown-unknown/release/voxam_wasm.wasm"

for shape in no-modules nodejs; do
    wasm-bindgen --target "$shape" --out-dir "$out/$shape" "$built"
done

echo "built into $out:"
echo "  no-modules/  the page's own: load voxam_wasm.js with a <script> tag"
echo "  nodejs/      the sweeps' own: required by certify/wasm_subject.js"
