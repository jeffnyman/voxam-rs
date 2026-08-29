"""The float oracle: golden vectors from the reference math layer.

The port's floats.rs pins these. The interesting edges: the
saturating integer conversions (with glulxe's 2147483647.0
boundary in both directions and Python's round-half-even for the
nearest variant), the fmod remainder/quotient pairs, pow's
guaranteed special cases, and the jfeq closeness test's infinity
rules.

Run from anywhere; VOXAM_REFERENCE names the reference checkout.
"""

import os
import sys
from pathlib import Path

reference = Path(
    os.environ.get("VOXAM_REFERENCE", Path(__file__).resolve().parent.parent.parent / "voxam")
)
sys.path.insert(0, str(reference / "src"))

from voxam.glulx.floats import (  # noqa: E402
    _close,
    _modulo,
    _pow,
    _to_int,
    decode_float,
    encode_float,
)

print("== encode/decode float ==")
for value in (0.0, -0.0, 1.5, -2.25, 3.4e38, -3.4e39, 1e-45):
    print(f"  {value!r} -> 0x{encode_float(value):08X}")
for bits in (0x3F800000, 0xBF800000, 0x7F800000, 0x7FC00000, 0x00000001):
    print(f"  0x{bits:08X} -> {decode_float(bits)!r}")

print("== to_int saturation and rounding ==")
for value, nearest in [
    (0.5, True), (1.5, True), (2.5, True), (-0.5, True), (-1.5, True),
    (2.7, False), (-2.7, False),
    (2147483646.5, True), (2147483647.0, True), (2147483648.0, True),
    (-2147483647.5, True), (-2147483648.0, True), (-2147483649.0, True),
    (float("inf"), True), (float("-inf"), False), (float("nan"), True),
]:
    print(f"  to_int({value!r}, nearest={nearest}) -> 0x{_to_int(value, nearest=nearest):08X}")

print("== modulo pairs ==")
for a, b in [
    (7.5, 2.0), (-7.5, 2.0), (7.5, -2.0), (-7.5, -2.0),
    (1.0, float("inf")), (float("inf"), 2.0), (1.0, 0.0),
]:
    remainder, quotient = _modulo(a, b)
    print(f"  modulo({a!r}, {b!r}) -> ({remainder!r}, {quotient!r})")

print("== pow specials ==")
for base, exponent in [
    (1.0, float("nan")), (float("nan"), 0.0), (-1.0, float("inf")),
    (0.0, -3.0), (-0.0, -3.0), (-0.0, -2.0), (-2.0, 3.0), (1e300, 3.0), (-1e300, 3.0),
    (-2.0, 0.5),
]:
    print(f"  pow({base!r}, {exponent!r}) -> {_pow(base, exponent)!r}")

print("== close (jfeq) ==")
for a, b, eps in [
    (1.0, 1.05, 0.1), (1.0, 1.2, 0.1), (1.0, 1.1, -0.1),
    (float("inf"), float("inf"), 0.0), (float("inf"), float("-inf"), float("inf")),
    (1.0, 2.0, float("inf")), (float("nan"), float("nan"), float("inf")),
]:
    print(f"  close({a!r}, {b!r}, {eps!r}) -> {_close(a, b, eps)}")
