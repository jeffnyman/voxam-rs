#!/bin/sh
# The Glulx machine parity sweep: a byte-exact save vector, then
# every checker .ulx booted with no Glk library and run until it
# quits or halts -- step counts and halting errors diffed whole
# against the reference.
#
# Usage and contract as header-diff.sh: 0 identical, 1 parted, 2
# unusable; VOXAM_REFERENCE names the reference checkout.

set -u

root=$(cd "$(dirname "$0")/.." && pwd)
reference=${VOXAM_REFERENCE:-"$root/../voxam"}
corpus="$root/entharion/glulx-checkers"

if [ ! -f "$reference/pyproject.toml" ]; then
    echo "certify: no reference implementation at $reference" >&2
    echo "certify: point VOXAM_REFERENCE at the Python voxam checkout" >&2
    exit 2
fi

if [ ! -d "$corpus" ]; then
    echo "certify: no checker corpus at $corpus; is the entharion submodule initialized?" >&2
    exit 2
fi

if ! cargo build --quiet --example glulx_machine --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

ported="$root/target/debug/examples/glulx_machine"
oracle="$root/certify/glulx_machine_reference.py"

reference_out=$(mktemp)
ported_out=$(mktemp)
trap 'rm -f "$reference_out" "$ported_out"' EXIT

(cd "$reference" && PYTHONUTF8=1 uv run --quiet python "$oracle" "$corpus") >"$reference_out" 2>&1
"$ported" "$corpus" >"$ported_out" 2>&1

if diff --strip-trailing-cr -q "$reference_out" "$ported_out" >/dev/null; then
    total=$(wc -l <"$ported_out")
    echo "certify: the save vector and $((total - 1)) bare runs identical"
    exit 0
fi

echo "PARTED: the bare machines disagree"
diff --strip-trailing-cr "$reference_out" "$ported_out" | head -20
exit 1
