#!/bin/sh
# The M3 gate itself: glulxercise, Glulx's own conformance suite,
# run whole under a scripted Glk display. The verdict is the
# story's to give -- "All tests passed." -- and nothing less
# counts.
#
# Exit 0 when every test passes, 1 when any fails, 2 when the
# gate cannot run.

set -u

root=$(cd "$(dirname "$0")/.." && pwd)
story="$root/entharion/glulx-checkers/glulxercise-r13-s241202.ulx"

if [ ! -f "$story" ]; then
    echo "certify: no glulxercise at $story; is the entharion submodule initialized?" >&2
    exit 2
fi

if ! cargo build --quiet --example glulxercise_probe --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

out=$(mktemp)
trap 'rm -f "$out"' EXIT

"$root/target/debug/examples/glulxercise_probe" "$story" all >"$out" 2>&1

if grep -q "All tests passed." "$out"; then
    echo "certify: glulxercise says: All tests passed."
    exit 0
fi

echo "PARTED: glulxercise did not pass whole"
grep -iE "fail|wrong" "$out" | head -8
exit 1
