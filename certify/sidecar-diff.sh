#!/bin/sh
# The sidecar parity sweep: the three wire sweeps run again with
# the "voxam" support token granted, so every update of every
# session carries the sidecar block -- location, score, turns, the
# delivered command, and the discontinuity bit -- and the
# transcripts still diff byte for byte (PORT: What the sidecar
# carries).
#
# Usage and contract as header-diff.sh: 0 identical, 1 parted, 2
# unusable; VOXAM_REFERENCE names the reference checkout.

set -u

root=$(cd "$(dirname "$0")/.." && pwd)

export VOXAM_SIDECAR=1

failed=0

for sweep in zglkote-diff gglkote-diff aaglkote-diff; do
    echo "== $sweep, sidecar granted"

    sh "$root/certify/$sweep.sh" || failed=1
done

if [ "$failed" -eq 0 ]; then
    echo "certify: every sidecar-granted wire telling identical"
    exit 0
fi

echo "PARTED: the sidecar's telling"
exit 1
