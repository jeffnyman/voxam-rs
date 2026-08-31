#!/bin/sh
# The linked host's parity sweep: every Z acceptance recording
# driven through the port twice -- once over real stdio, once over
# the in-process pipes the desktop shell links the interpreter
# with -- and the update transcripts diffed byte for byte.
#
# The subprocess the shell used to spawn was the operating
# system's pipe; a linked session is a pipe of our own making, and
# this sweep is what says the two are the same wire. The stdio
# subject is `zglkote`, which the Z wire sweep certifies against
# the Python reference, so an identical run here inherits that
# certification whole.
#
# Usage:
#   certify/linked-diff.sh [recording.accept...]
#
# With no arguments, sweeps every Z recording in the reference's
# acceptance/ directory. Exit 0 when nothing parted, 1 on any real
# divergence, 2 when the sweep cannot run. VOXAM_REFERENCE names
# the reference checkout (default: ../voxam), whose acceptance
# recordings and stories the drive reads.

set -u

root=$(cd "$(dirname "$0")/.." && pwd)
reference=${VOXAM_REFERENCE:-"$root/../voxam"}

if [ ! -f "$reference/pyproject.toml" ]; then
    echo "certify: no reference implementation at $reference" >&2
    echo "certify: point VOXAM_REFERENCE at the Python voxam checkout" >&2
    exit 2
fi

if ! cargo build --quiet --example zglkote --example linked \
    --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

stdio="$root/target/debug/examples/zglkote"
linked="$root/target/debug/examples/linked"

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

    mkdir -p "$work/stdio-$name" "$work/linked-$name"

    (cd "$reference" && PYTHONUTF8=1 uv run --quiet python \
        "$root/certify/zglkote_drive.py" "$recording" "$work/stdio-$name" -- \
        "$stdio" "$story" $seed) >"$work/$name.stdio" 2>&1

    (cd "$reference" && PYTHONUTF8=1 uv run --quiet python \
        "$root/certify/zglkote_drive.py" "$recording" "$work/linked-$name" -- \
        "$linked" "$story" $seed) >"$work/$name.linked" 2>&1

    if diff --strip-trailing-cr -q "$work/$name.stdio" "$work/$name.linked" >/dev/null; then
        echo "IDENTICAL: $name"
        identical=$((identical + 1))
    else
        echo "PARTED: $name"
        diff --strip-trailing-cr "$work/$name.stdio" "$work/$name.linked" | head -8
        parted=$((parted + 1))
    fi
done

echo "certify: $swept linked sessions -- $identical identical, $parted parted"

[ "$parted" -eq 0 ] || exit 1
exit 0
