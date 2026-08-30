"""The reference side of the gallery parity sweep.

Hangs one Blorb's gallery and prints its census: the release
number, the Reso chunk's standard window and each scaling entry's
exact fractions, the adaptive picture numbers, the baked
replacement records, and every picture's size beside its scaling
ratio on a roomy 640-by-400 screen. Exercises the chunk parsers
and the Elbow Room arithmetic over the real Version 6 art sets.
"""

import sys
from fractions import Fraction
from pathlib import Path

from voxam.blorb import Blorb


def spelled(ratio: Fraction | None) -> str:
    if ratio is None:
        return "-"

    return f"{ratio.numerator}/{ratio.denominator}"


def main() -> int:
    blorb = Blorb.parse(Path(sys.argv[1]).read_bytes())
    gallery = blorb.gallery()

    print(f"release {gallery.release}")

    if blorb.resolution is not None:
        held = blorb.resolution

        print(f"window {held.width}x{held.height}")

        for number in sorted(held.scalings):
            scaling = held.scalings[number]

            print(
                f"scaling {number} {spelled(scaling.standard)} "
                f"{spelled(scaling.minimum)} {spelled(scaling.maximum)}"
            )

    if blorb.adaptive:
        print("adaptive", " ".join(str(number) for number in sorted(blorb.adaptive)))

    for scene, adaptive in sorted(blorb.baked):
        print(f"baked {scene} {adaptive} {blorb.baked[(scene, adaptive)]}")

    for number in sorted(gallery._art):  # noqa: SLF001 -- the census walks every seat
        height, width = gallery.size(number)
        ratio = gallery.scale(number, 640, 400)

        print(f"pict {number} {height}x{width} {spelled(ratio)}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
