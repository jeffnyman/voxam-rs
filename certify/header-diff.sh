#!/bin/sh
# The header parity sweep: render every Z-code story's --header
# report under both implementations and diff them.
#
# The Python implementation is the reference; this port must render
# identically, byte for byte, with one dispensation: the reference's
# stdout on Windows wears CRLF line endings by the platform's
# text-mode manners, so the diff strips trailing carriage returns.
#
# Usage:
#   certify/header-diff.sh [story...]
#
# With no arguments, sweeps every *.z1 through *.z8 under the
# entharion submodule. VOXAM_REFERENCE names the reference checkout
# (default: ../voxam, the sibling repository).
#
# The exit code speaks RegTest's contract: 0 for identical reports,
# 1 for reports that part, 2 when the sweep cannot run at all.

set -u

root=$(cd "$(dirname "$0")/.." && pwd)
reference=${VOXAM_REFERENCE:-"$root/../voxam"}

if [ ! -f "$reference/pyproject.toml" ]; then
    echo "certify: no reference implementation at $reference" >&2
    echo "certify: point VOXAM_REFERENCE at the Python voxam checkout" >&2
    exit 2
fi

if ! cargo build --quiet --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

ported="$root/target/debug/voxam"

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
    # The reference runs from its own checkout, so a story named
    # relative to the caller must travel as an absolute path.
    case $story in
    /*) ;;
    *) story="$caller/$story" ;;
    esac

    total=$((total + 1))

    (cd "$reference" && uv run --quiet voxam --header "$story") >"$reference_out" 2>&1
    "$ported" --header "$story" >"$ported_out" 2>&1

    if ! diff --strip-trailing-cr -q "$reference_out" "$ported_out" >/dev/null; then
        parted=$((parted + 1))
        echo "PARTED: $story"
        diff --strip-trailing-cr "$reference_out" "$ported_out" | head -12
    fi
done

if [ "$parted" -eq 0 ]; then
    echo "certify: $total stories, every report identical"
    exit 0
fi

echo "certify: $parted of $total reports parted"
exit 1
