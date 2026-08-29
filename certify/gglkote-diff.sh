#!/bin/sh
# The Glulx wire parity sweep: every Glulx acceptance recording
# driven through both implementations' GlkOte serving loops by the
# same deterministic typist (zglkote_drive.py), the update
# transcripts diffed byte for byte -- the zglkote sweep's twin on
# the other machine.
#
# Usage:
#   certify/gglkote-diff.sh [recording.accept...]
#
# With no arguments, sweeps every Glulx recording in the
# reference's acceptance/ directory. Exit 0 when nothing parted, 1
# on any real divergence, 2 when the sweep cannot run.
# VOXAM_REFERENCE names the reference checkout (default: ../voxam).

set -u

root=$(cd "$(dirname "$0")/.." && pwd)
reference=${VOXAM_REFERENCE:-"$root/../voxam"}

if [ ! -f "$reference/pyproject.toml" ]; then
    echo "certify: no reference implementation at $reference" >&2
    echo "certify: point VOXAM_REFERENCE at the Python voxam checkout" >&2
    exit 2
fi

if ! cargo build --quiet --example gglkote --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

ported="$root/target/debug/examples/gglkote"

if [ $# -gt 0 ]; then
    recordings="$*"
else
    recordings=$(grep -l 'GAME=.*\.\(ulx\|gblorb\)' "$reference"/acceptance/*.accept | sort)
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

    mkdir -p "$work/ref-$name" "$work/port-$name"

    (cd "$reference" && PYTHONUTF8=1 uv run --quiet python \
        "$root/certify/zglkote_drive.py" "$recording" "$work/ref-$name" -- \
        uv run --quiet python "$root/certify/gglkote_reference.py" \
        "$story" $seed) >"$work/$name.ref" 2>&1

    (cd "$reference" && PYTHONUTF8=1 uv run --quiet python \
        "$root/certify/zglkote_drive.py" "$recording" "$work/port-$name" -- \
        "$ported" "$story" $seed) >"$work/$name.port" 2>&1

    if diff --strip-trailing-cr -q "$work/$name.ref" "$work/$name.port" >/dev/null; then
        echo "IDENTICAL: $name"
        identical=$((identical + 1))
    else
        echo "PARTED: $name"
        diff --strip-trailing-cr "$work/$name.ref" "$work/$name.port" | head -8
        parted=$((parted + 1))
    fi
done

echo "certify: $swept sessions -- $identical identical, $parted parted"

[ "$parted" -eq 0 ] || exit 1
exit 0
