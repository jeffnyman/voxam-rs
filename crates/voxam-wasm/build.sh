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

# The playable demo: the module, the adapter, the page, and the
# display the CLI's browser face already vendors -- gathered where
# a static server can reach them all.
demo="$out/demo"

mkdir -p "$demo"
cp "$out/no-modules/voxam_wasm.js" "$out/no-modules/voxam_wasm_bg.wasm" "$demo/"
cp "$here/demo/index.html" "$here/demo/voxam-glkote.js" "$demo/"

for asset in glkote.js glkote.css jquery-1.12.4.min.js voxam-audio.js waiting.gif; do
    cp "$root/crates/voxam/pages/$asset" "$demo/"
done

echo "built into $out:"
echo "  no-modules/  the page's own: load voxam_wasm.js with a <script> tag"
echo "  nodejs/      the sweeps' own: required by certify/wasm_subject.js"
echo "  demo/        a page that plays. Serve it and open it:"
echo ""
echo "      python -m http.server -d $demo 8000"
echo ""
echo "  then http://localhost:8000 and choose a story file."
