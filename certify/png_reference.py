"""The reference side of the PNG parity sweep.

Walks one Blorb's Pict resources in index order and, for every PNG
aboard, decodes it through the adaptive-free path and re-encodes
the pixels -- the very transform the Version 6 wire performs before
a picture rides as a data: url. The encoded bytes print as
hexadecimal, so the diff compares the decoded pixels, the clear
flags, the alpha channel, and the hand-spelled deflate stream all
at once. Non-PNG picts (JPEG, Rect placeholders) print as skipped
so both censuses stay aligned; a PNG the decoder refuses prints its
complaint.
"""

import sys
from pathlib import Path

from voxam.blorb import Blorb
from voxam.errors import VoxamError
from voxam.png import SIGNATURE, decode, encoded


def main() -> int:
    data = Path(sys.argv[1]).read_bytes()
    blorb = Blorb.parse(data)

    for resource in blorb.resources:
        if resource.usage != b"Pict":
            continue

        payload = bytes(resource.chunk.payload)

        if not payload.startswith(SIGNATURE):
            print(f"pict {resource.number} skipped")
            continue

        try:
            picture = decode(payload)
        except VoxamError as error:
            print(f"pict {resource.number} refused: {error}")
            continue

        print(f"pict {resource.number} {encoded(picture).hex()}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
