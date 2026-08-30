#!/bin/sh
# The stage parity sweep: eleven drills through the §8.8 StageModel
# under both implementations -- boot wrap, bottom scrolls, dressed
# and placed windows, the split dance, margins with anchored-margin
# flow scrolls, the erases, §15 scroll_window, line editing, the
# [MORE] budget, odd font metrics, and the refusals -- the grids,
# cursors, sweeps, unit paints, and pauses diffed line for line.
#
# Usage and contract as header-diff.sh: 0 identical, 1 parted, 2
# unusable; VOXAM_REFERENCE names the reference checkout.

set -u

root=$(cd "$(dirname "$0")/.." && pwd)
reference=${VOXAM_REFERENCE:-"$root/../voxam"}

if [ ! -f "$reference/pyproject.toml" ]; then
    echo "certify: no reference implementation at $reference" >&2
    echo "certify: point VOXAM_REFERENCE at the Python voxam checkout" >&2
    exit 2
fi

if ! cargo build --quiet --example stage_parity --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

reference_out=$(mktemp)
ported_out=$(mktemp)
trap 'rm -f "$reference_out" "$ported_out"' EXIT

(cd "$reference" && PYTHONUTF8=1 uv run --quiet python "$root/certify/stage_reference.py") >"$reference_out" 2>&1
"$root/target/debug/examples/stage_parity" >"$ported_out" 2>&1

if diff --strip-trailing-cr -q "$reference_out" "$ported_out" >/dev/null; then
    echo "certify: 11 stage drills, every telling identical"
    exit 0
fi

echo "PARTED: the stage's telling"
diff --strip-trailing-cr "$reference_out" "$ported_out" | head -20
exit 1
