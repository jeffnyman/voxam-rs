#!/bin/sh
# The Å-machine terminal parity sweep: every .aastory fixture
# played through the terminal face under both implementations with
# the same scripted streams -- Cloak of Darkness on a written
# walkthrough, the others on their own vendored input scripts --
# and the sessions diffed whole.
#
# Usage and contract as header-diff.sh: 0 identical, 1 parted, 2
# unusable; VOXAM_REFERENCE names the reference checkout.

set -u

root=$(cd "$(dirname "$0")/.." && pwd)
reference=${VOXAM_REFERENCE:-"$root/../voxam"}
fixtures="$reference/tests/fixtures"

if [ ! -f "$reference/pyproject.toml" ]; then
    echo "certify: no reference implementation at $reference" >&2
    echo "certify: point VOXAM_REFERENCE at the Python voxam checkout" >&2
    exit 2
fi

if ! cargo build --quiet --example aaterminal --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

ported="$root/target/debug/examples/aaterminal"
oracle="$root/certify/aaterminal_reference.py"

reference_out=$(mktemp)
ported_out=$(mktemp)
cloak_in=$(mktemp)
trap 'rm -f "$reference_out" "$ported_out" "$cloak_in"' EXIT

# Cloak of Darkness, walked whole: the dark bar, the hook, the
# message, and a polite quit.
cat >"$cloak_in" <<'SCRIPT'
inventory
east
south
read message
north
west
hang cloak on hook
east
south
read message
score
quit
y
SCRIPT

total=0
parted=0

session() {
    name=$1
    story=$2
    script=$3
    total=$((total + 1))

    (cd "$reference" && PYTHONUTF8=1 uv run --quiet python "$oracle" "$story" "$script") >"$reference_out" 2>&1
    "$ported" "$story" "$script" >"$ported_out" 2>&1

    if diff --strip-trailing-cr -q "$reference_out" "$ported_out" >/dev/null; then
        echo "IDENTICAL: $name"
    else
        parted=$((parted + 1))
        echo "PARTED: $name"
        diff --strip-trailing-cr "$reference_out" "$ported_out" | head -12
    fi
}

session cloak-rel2 "$fixtures/cloak-rel2.aastory" "$cloak_in"
session gosling "$fixtures/gosling.aastory" "$fixtures/gosling.in"
session body_not_status "$fixtures/body_not_status.aastory" "$fixtures/body_not_status.in"
session codepoints "$fixtures/codepoints.aastory" "$fixtures/codepoints.in"

if [ "$parted" -eq 0 ]; then
    echo "certify: $total sessions, every terminal telling identical"
    exit 0
fi

echo "certify: $parted of $total sessions parted"
exit 1
