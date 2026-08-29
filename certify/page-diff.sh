#!/bin/sh
# The GlkOte page parity sweep: four update drills -- the first
# tree, styled content with a posted field, a regenerated field
# carrying typed text, and a full spread of grid, canvas, timer,
# and file ask -- built by both implementations' Page and diffed
# as the compact JSON the wire actually speaks.
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

if ! cargo build --quiet --example page_probe --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

reference_out=$(mktemp)
ported_out=$(mktemp)
trap 'rm -f "$reference_out" "$ported_out"' EXIT

(cd "$reference" && PYTHONUTF8=1 uv run --quiet python "$root/certify/page_reference.py") >"$reference_out" 2>&1
"$root/target/debug/examples/page_probe" >"$ported_out" 2>&1

if diff --strip-trailing-cr -q "$reference_out" "$ported_out" >/dev/null; then
    echo "certify: 4 update drills, every stanza identical"
    exit 0
fi

echo "PARTED: the page's stanzas"
diff --strip-trailing-cr "$reference_out" "$ported_out" | head -12
exit 1
