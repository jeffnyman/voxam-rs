#!/bin/sh
# The replay parity sweep: replay every acceptance recording under
# both implementations and diff the sessions.
#
# Three verdicts per recording. IDENTICAL: the whole session,
# byte for byte, refusal warnings included. FRONTIER: the port
# halted at a named not-yet-implemented feature -- and everything
# it printed up to that halt is a byte prefix of the reference's
# session, so the halt is honest and the machine agreed as far as
# it went. PARTED: real divergence, which is the only failure.
#
# Two dispensations, both documented departures rather than bugs:
# trailing carriage returns are stripped (the reference's Windows
# text mode), and the reference's "Resources:" sidecar line -- the
# blorb census the port has not grown yet -- is filtered with its
# following blank line, to be retired with the blorb module.
#
# Usage:
#   certify/replay-diff.sh [recording.accept...]
#
# With no arguments, sweeps every recording in the reference's
# acceptance/ directory. VOXAM_REFERENCE names the reference
# checkout (default: ../voxam). Exit 0 when nothing parted, 1 on
# any real divergence, 2 when the sweep cannot run.

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
    recordings="$*"
else
    recordings=$(find "$reference/acceptance" -name "*.accept" | sort)
fi

if [ -z "$recordings" ]; then
    echo "certify: no recordings to sweep" >&2
    exit 2
fi

reference_out=$(mktemp)
ported_out=$(mktemp)
ported_prefix=$(mktemp)
reference_head=$(mktemp)
trap 'rm -f "$reference_out" "$ported_out" "$ported_prefix" "$reference_head"' EXIT

caller=$PWD
total=0
identical=0
frontier=0
parted=0

for recording in $recordings; do
    case $recording in
    /*) ;;
    *) recording="$caller/$recording" ;;
    esac

    total=$((total + 1))
    name=$(basename "$recording" .accept)

    (cd "$reference" && PYTHONUTF8=1 uv run --quiet voxam --plain --accept "$recording") \
        2>&1 | tr -d '\r' | sed -e '/^Resources: /{N;d;}' >"$reference_out"
    "$ported" --accept "$recording" 2>&1 | tr -d '\r' >"$ported_out"

    if cmp -s "$reference_out" "$ported_out"; then
        identical=$((identical + 1))
        echo "IDENTICAL: $name"
        continue
    fi

    # A frontier halt ends the port's output with a voxam: line
    # naming what was reached; everything before it must be a byte
    # prefix of the reference's session.
    reason=$(tail -n 1 "$ported_out" | grep -oE \
        "reached .*, which is not yet implemented|needs the keystroke seam|story file declares version [0-9]+.*|only versions 1 to 8 exist.*")

    if [ -n "$reason" ]; then
        head -n -1 "$ported_out" | sed -e '${/^$/d}' >"$ported_prefix"
        head -c "$(wc -c <"$ported_prefix")" "$reference_out" >"$reference_head"

        if cmp -s "$reference_head" "$ported_prefix"; then
            frontier=$((frontier + 1))
            echo "FRONTIER: $name ($reason)"
            continue
        fi
    fi

    parted=$((parted + 1))
    echo "PARTED: $name"
    diff "$reference_out" "$ported_out" | head -8
done

echo "certify: $total recordings -- $identical identical, $frontier at the frontier, $parted parted"

if [ "$parted" -eq 0 ]; then
    exit 0
fi

exit 1
