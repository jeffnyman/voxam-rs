#!/bin/sh
# The PNG parity sweep: decode and re-encode every PNG picture of
# every Blorb in the corpus under both implementations and diff
# them.
#
# This is the Version 6 wire's own transform -- a plotted picture
# rides as a data: url of re-encoded pixels -- so the sweep proves
# the whole road at once: chunk walking, inflation of streams other
# tools compressed, scanline unfiltering, pixel extraction, and the
# hand-spelled deterministic deflate both implementations write.
#
# Usage and contract as header-diff.sh: 0 identical, 1 parted, 2
# unusable; VOXAM_REFERENCE names the reference checkout. The pure
# Python side decodes a few megabytes of art pixel by pixel, so the
# sweep takes minutes, not seconds.

set -u

root=$(cd "$(dirname "$0")/.." && pwd)
reference=${VOXAM_REFERENCE:-"$root/../voxam"}

if [ ! -f "$reference/pyproject.toml" ]; then
    echo "certify: no reference implementation at $reference" >&2
    echo "certify: point VOXAM_REFERENCE at the Python voxam checkout" >&2
    exit 2
fi

if ! cargo build --quiet --example png_parity --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

ported="$root/target/debug/examples/png_parity"
oracle="$root/certify/png_reference.py"

if [ $# -gt 0 ]; then
    blorbs="$*"
else
    blorbs=$(find "$root/entharion" -name "*.blb" -o -name "*.blorb" -o -name "*.zblorb" | sort)
fi

if [ -z "$blorbs" ]; then
    echo "certify: no blorbs to sweep; is the entharion submodule initialized?" >&2
    exit 2
fi

reference_out=$(mktemp)
ported_out=$(mktemp)
trap 'rm -f "$reference_out" "$ported_out"' EXIT

caller=$PWD
total=0
parted=0

for blorb in $blorbs; do
    case $blorb in
    /*) ;;
    *) blorb="$caller/$blorb" ;;
    esac

    total=$((total + 1))

    (cd "$reference" && PYTHONUTF8=1 uv run --quiet python "$oracle" "$blorb") >"$reference_out" 2>&1
    "$ported" "$blorb" >"$ported_out" 2>&1

    if ! diff --strip-trailing-cr -q "$reference_out" "$ported_out" >/dev/null; then
        parted=$((parted + 1))
        echo "PARTED: $blorb"
        diff --strip-trailing-cr "$reference_out" "$ported_out" | head -6
    fi
done

if [ "$parted" -eq 0 ]; then
    echo "certify: $total blorbs, every picture identical"
    exit 0
fi

echo "certify: $parted of $total blorbs parted"
exit 1
