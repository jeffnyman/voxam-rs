#!/bin/sh
# The browser build's parity sweep: every Z acceptance recording
# driven through the port twice -- once over real stdio, once
# through the wasm module a page would load -- and the update
# transcripts diffed byte for byte.
#
# This is what makes the browser face certified rather than merely
# plausible. The module speaks stanzas and nothing else, so the
# same typist that drives the stdio subject drives it too; the
# stdio subject is `zglkote`, which the Z wire sweep certifies
# against the Python reference, so an identical run here inherits
# that certification whole.
#
# Usage:
#   certify/wasm-diff.sh [recording.accept...]
#
# With no arguments, sweeps every Z recording in the reference's
# acceptance/ directory. Exit 0 when nothing parted, 1 on any real
# divergence, 2 when the sweep cannot run. VOXAM_REFERENCE names
# the reference checkout (default: ../voxam).
#
# Needs the wasm target, a matching wasm-bindgen CLI, and node:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <the crate's version>

set -u

root=$(cd "$(dirname "$0")/.." && pwd)
reference=${VOXAM_REFERENCE:-"$root/../voxam"}

if [ ! -f "$reference/pyproject.toml" ]; then
    echo "certify: no reference implementation at $reference" >&2
    echo "certify: point VOXAM_REFERENCE at the Python voxam checkout" >&2
    exit 2
fi

if ! command -v node >/dev/null 2>&1; then
    echo "certify: node is needed to drive the browser build" >&2
    exit 2
fi

if ! command -v wasm-bindgen >/dev/null 2>&1; then
    echo "certify: wasm-bindgen is not on the PATH" >&2
    echo "certify: cargo install wasm-bindgen-cli --version <the crate's version>" >&2
    exit 2
fi

if ! cargo build --quiet --example zglkote --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

# The module, built for the browser and bound for node. The node
# target is the one this harness can require; the page ships the
# no-modules build, from these very same bytes.
if ! cargo build --quiet --release --target wasm32-unknown-unknown \
    --manifest-path "$root/crates/voxam-wasm/Cargo.toml"; then
    echo "certify: the browser build does not build" >&2
    exit 2
fi

glue="$root/target/wasm-certify"
rm -rf "$glue"

if ! wasm-bindgen --target nodejs --out-dir "$glue" \
    "$root/crates/voxam-wasm/target/wasm32-unknown-unknown/release/voxam_wasm.wasm"; then
    echo "certify: the browser build could not be bound" >&2
    exit 2
fi

stdio="$root/target/debug/examples/zglkote"

if [ $# -gt 0 ]; then
    recordings="$*"
else
    recordings=$(grep -l 'GAME=.*\.z' "$reference"/acceptance/*.accept | sort)
fi

if [ -z "$recordings" ]; then
    echo "certify: no recordings to sweep" >&2
    exit 2
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

identical=0
parted=0
swept=0

for recording in $recordings; do
    name=$(basename "$recording" .accept)
    game=$(sed -n 's/^! GAME=//p' "$recording" | head -1)

    seed=$(sed -n 's/^! SEED=//p' "$recording" | head -1)
    story="$reference/acceptance/$game"
    swept=$((swept + 1))

    mkdir -p "$work/stdio-$name" "$work/wasm-$name"

    (cd "$reference" && PYTHONUTF8=1 uv run --quiet python \
        "$root/certify/zglkote_drive.py" "$recording" "$work/stdio-$name" -- \
        "$stdio" "$story" $seed) >"$work/$name.stdio" 2>&1

    (cd "$reference" && PYTHONUTF8=1 uv run --quiet python \
        "$root/certify/zglkote_drive.py" "$recording" "$work/wasm-$name" -- \
        node "$root/certify/wasm_subject.js" "$glue/voxam_wasm.js" \
        "$story" $seed) >"$work/$name.wasm" 2>&1

    if diff --strip-trailing-cr -q "$work/$name.stdio" "$work/$name.wasm" >/dev/null; then
        echo "IDENTICAL: $name"
        identical=$((identical + 1))
    else
        echo "PARTED: $name"
        diff --strip-trailing-cr "$work/$name.stdio" "$work/$name.wasm" | head -8
        parted=$((parted + 1))
    fi
done

echo "certify: $swept browser sessions -- $identical identical, $parted parted"

[ "$parted" -eq 0 ] || exit 1
exit 0
