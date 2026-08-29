"""The RNG oracle: golden vectors from the reference Randomizer.

The port's rng.rs pins these exact values as its compatibility
contract -- the same stance the reference's own test suite takes:
a change here is a breaking change, not a test to update. Rerun
this to regenerate the vectors if the contract ever must move, or
to verify the reference still speaks them.

Run from anywhere; VOXAM_REFERENCE names the reference checkout
(default: the voxam sibling of this repository).
"""

import os
import sys
from pathlib import Path

reference = Path(
    os.environ.get("VOXAM_REFERENCE", Path(__file__).resolve().parent.parent.parent / "voxam")
)
sys.path.insert(0, str(reference / "src"))

from voxam.zmachine.rng import Randomizer, _mixed  # noqa: E402

print("mixed:")
for value in [0, 1, 3, 42, 999, 1000, 1137, 5000, 0x7FFFFFFF, 0xFFFFFFFF]:
    print(f"  ({value}, 0x{_mixed(value):08X}),")

raw = Randomizer(seed=1137)
print("raw stream, session seed 1137:")
print("  " + ", ".join(f"0x{raw._next():08X}" for _ in range(10)))

for seed in (1137, 42):
    rng = Randomizer(seed=seed)
    print(f"roll(100) x20, session seed {seed}:")
    print("  " + ", ".join(str(rng.roll(100)) for _ in range(20)))

rng = Randomizer()
rng.seed(5000)
print("roll(100) x20, opcode seed 5000:")
print("  " + ", ".join(str(rng.roll(100)) for _ in range(20)))

rng = Randomizer(seed=42)
print("roll(6) x20, session seed 42:")
print("  " + ", ".join(str(rng.roll(6)) for _ in range(20)))
