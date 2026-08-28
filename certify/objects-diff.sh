#!/bin/sh
# The object table parity sweep: walk every object of every Z-code
# story -- relations, attributes, short names, properties -- under
# both implementations and diff them.
#
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

if ! cargo build --quiet --example objects --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

ported="$root/target/debug/examples/objects"
oracle="$root/certify/objects_reference.py"

if [ $# -gt 0 ]; then
    stories="$*"
else
    stories=$(find "$root/entharion" -name "*.z[1-8]" | sort)
fi

if [ -z "$stories" ]; then
    echo "certify: no stories to sweep; is the entharion submodule initialized?" >&2
    exit 2
fi

reference_out=$(mktemp)
ported_out=$(mktemp)
trap 'rm -f "$reference_out" "$ported_out"' EXIT

caller=$PWD
total=0
parted=0

for story in $stories; do
    case $story in
    /*) ;;
    *) story="$caller/$story" ;;
    esac

    total=$((total + 1))

    (cd "$reference" && PYTHONUTF8=1 uv run --quiet python "$oracle" "$story") >"$reference_out" 2>&1
    "$ported" "$story" >"$ported_out" 2>&1

    if ! diff --strip-trailing-cr -q "$reference_out" "$ported_out" >/dev/null; then
        parted=$((parted + 1))
        echo "PARTED: $story"
        diff --strip-trailing-cr "$reference_out" "$ported_out" | head -12
    fi
done

if [ "$parted" -eq 0 ]; then
    echo "certify: $total stories, every object table identical"
    exit 0
fi

echo "certify: $parted of $total stories parted"
exit 1
