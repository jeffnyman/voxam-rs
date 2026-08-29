#!/bin/sh
# The M4 gate itself: the reference's vendored gold transcripts --
# the community fork's own engine at seed 1234 -- replayed under
# the port's plain voice and compared byte for byte. aa-exercise
# stresses every opcode (twice: plain, and with the SAVEFILE
# feature declared), gosling is a 351-line walk through a real
# Dialog game, body_not_status is format 1.0, and codepoints walks
# the character set, the keypress loop, and the progress bars.
#
# Exit 0 when every walk matches whole, 1 when any parts, 2 when
# the gate cannot run. VOXAM_REFERENCE names the reference
# checkout whose fixtures hold the stories and golds.

set -u

root=$(cd "$(dirname "$0")/.." && pwd)
reference=${VOXAM_REFERENCE:-"$root/../voxam"}
fixtures="$reference/tests/fixtures"

if [ ! -d "$fixtures" ]; then
    echo "certify: no fixtures at $fixtures" >&2
    echo "certify: point VOXAM_REFERENCE at the Python voxam checkout" >&2
    exit 2
fi

if ! cargo build --quiet --example aawalk --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

ported="$root/target/debug/examples/aawalk"
out=$(mktemp)
trap 'rm -f "$out"' EXIT

total=0
parted=0

walk() {
    name=$1
    gold=$2
    shift 2
    total=$((total + 1))

    "$ported" "$@" >"$out" 2>&1

    # The reference compares transcripts through universal
    # newlines, so the gold's stray CRLF echo lines read as LF --
    # the same dispensation the replay sweep grants.
    if diff --strip-trailing-cr -q "$fixtures/$gold.gold" "$out" >/dev/null; then
        echo "IDENTICAL: $name"
    else
        parted=$((parted + 1))
        echo "PARTED: $name"
        diff --strip-trailing-cr "$fixtures/$gold.gold" "$out" | head -12
    fi
}

walk aa-exercise aa-exercise "$fixtures/aa-exercise.aastory"
walk aa-exercise-saves aa-exercise-saves "$fixtures/aa-exercise.aastory" --saves
walk gosling gosling "$fixtures/gosling.aastory" "$fixtures/gosling.in"
walk body_not_status body_not_status \
    "$fixtures/body_not_status.aastory" "$fixtures/body_not_status.in"
walk codepoints codepoints "$fixtures/codepoints.aastory" "$fixtures/codepoints.in"

if [ "$parted" -eq 0 ]; then
    echo "certify: $total walks, every transcript identical to the gold"
    exit 0
fi

echo "certify: $parted of $total walks parted"
exit 1
