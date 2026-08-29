"""The ZSCII oracle: golden vectors from the reference decoder.

The port's zscii.rs pins these decode results, encode bytes, and
error messages as test vectors. Every case builds the reference
test suite's own 512-byte image shape and runs the genuine
implementation, so a vector can never be a guess -- the method
that caught two hand-crafted mistakes before they became wrong
fixtures. Rerun this to verify the reference still speaks them,
or to grow the battery.

Run from anywhere; VOXAM_REFERENCE names the reference checkout
(default: the voxam sibling of this repository). Windows consoles
want PYTHONUTF8=1.
"""

import os
import sys
from pathlib import Path

reference = Path(
    os.environ.get("VOXAM_REFERENCE", Path(__file__).resolve().parent.parent.parent / "voxam")
)
sys.path.insert(0, str(reference / "src"))

from voxam.errors import ZMachineTextError  # noqa: E402
from voxam.zmachine.memory import Memory  # noqa: E402
from voxam.zmachine.story import Story  # noqa: E402
from voxam.zmachine.zscii import (  # noqa: E402
    DEFAULT_EXTRAS,
    alphabets,
    char_to_zscii,
    decode_string,
    encode_word,
    fuse_surrogates,
)

SIZE = 512


def plant(data: bytearray, at: int, chunk: bytes) -> None:
    """Exact-length placement; never resizes the bytearray."""
    assert at + len(chunk) <= len(data)
    data[at : at + len(chunk)] = chunk
    assert len(data) == SIZE


def base_image(version: int) -> bytearray:
    data = bytearray(SIZE)
    data[0] = version
    plant(data, 0x04, (0x01C0).to_bytes(2, "big"))
    plant(data, 0x0E, (0x01C0).to_bytes(2, "big"))
    return data


def memory_of(data: bytearray) -> Memory:
    return Memory(Story(bytes(data)))


def pack(zchars: list[int]) -> bytes:
    """Pack Z-characters into words, padded with 5s, terminator last."""
    padded = zchars + [5] * (-len(zchars) % 3)
    out = b""
    for i in range(0, len(padded), 3):
        word = (padded[i] << 10) | (padded[i + 1] << 5) | padded[i + 2]
        if i + 3 == len(padded):
            word |= 0x8000
        out += word.to_bytes(2, "big")
    return out


def a0(text: str) -> list[int]:
    return [6 + ord(c) - ord("a") for c in text]


def show(name: str, version: int, zchars: list[int]) -> None:
    data = base_image(version)
    plant(data, 0x80, pack(zchars))
    try:
        text, end = decode_string(memory_of(data), 0x80)
        print(f"  {name}: v{version} {zchars} -> {ascii(text)} end={end - 0x80}")
    except ZMachineTextError as e:
        print(f"  {name}: v{version} {zchars} -> ERROR {e}")


print("== decode cases ==")
show("lowercase+space", 3, a0("hello") + [0] + a0("world"))
show("single shift A1", 3, [4, 13, 14])
show("shift then space", 3, [4, 0, 22])
show("A2 digits", 3, [5, 8, 5, 9, 5, 18])
show("A2 newline", 3, [6, 5, 7, 7])
show("escape at-sign", 3, [5, 6, 2, 0, 0, 0])
show("escape extra 155", 3, [5, 6, 4, 27, 0, 0])
show("truncated escape ignored", 3, [6, 5, 6])
show("abbrev char as final zchar ignored", 3, [6, 6, 1])
show("v3 shift before space consumed", 3, [5, 0, 8])
show("null via escape prints nothing", 3, [6, 5, 6, 0, 0, 7, 0])
show("v1 newline zchar1", 1, [6, 1, 7])
show("v1 A2 char 21", 1, [3, 21, 0])
show("v1 relative shift up", 1, [2, 13, 14])
show("v1 relative shift down+char", 1, [3, 8, 31])
show("v1 lock down then chars", 1, [5, 8, 9, 10])
show("v2 lock A1 then chars", 2, [4, 6, 7, 5, 5, 8])
show("v2 temp shift", 2, [2, 13, 14])
show("v2 relative shift down", 2, [3, 8, 31])
show("v6 typography 9 via escape", 6, [6, 5, 6, 0, 9, 7, 0])
show("v6 typography 11 via escape", 6, [6, 5, 6, 0, 11, 7, 0])
show("ibm arrow 24 via escape", 5, [5, 6, 0, 24, 0, 0])

print("== custom alphabet table (v5, at $150) ==")
table = bytes(
    [ord(c) for c in "zyxwvutsrqponmlkjihgfedcba"]
    + [ord(c) for c in "0123456789.........,......"]
    + [0, 0]
    + [ord(c) for c in "ABCDEFGHIJKLMNOPQRSTUVWX"]
)


def custom_alphabet_case(name: str, zchars: list[int]) -> None:
    data = base_image(5)
    plant(data, 0x34, (0x0150).to_bytes(2, "big"))
    plant(data, 0x150, table)
    plant(data, 0x80, pack(zchars))
    text, _ = decode_string(memory_of(data), 0x80)
    print(f"  {name}: {zchars} -> {ascii(text)}")


custom_alphabet_case("rows redefined", [6, 4, 6, 5, 8, 0])
custom_alphabet_case("A2 escape+newline defy table", [5, 7, 6])
custom_alphabet_case("null slot prints nothing", [5, 8, 5, 9, 7])

print("== custom extras translation table (v5) ==")


def extras_image(entries: list[int]) -> bytearray:
    data = base_image(5)
    plant(data, 0x36, (0x0170).to_bytes(2, "big"))
    plant(data, 0x170, (3).to_bytes(2, "big"))  # extension word count
    plant(data, 0x176, (0x0180).to_bytes(2, "big"))  # word 3: unicode table
    plant(data, 0x180, bytes([len(entries)]))
    for index, code in enumerate(entries):
        plant(data, 0x181 + 2 * index, code.to_bytes(2, "big"))
    return data


def extras_case(name: str, entries: list[int], zchars: list[int]) -> None:
    data = extras_image(entries)
    plant(data, 0x80, pack(zchars))
    try:
        text, _ = decode_string(memory_of(data), 0x80)
        print(f"  {name} -> {ascii(text)} fused -> {ascii(fuse_surrogates(text))}")
    except ZMachineTextError as e:
        print(f"  {name} -> ERROR {e}")


extras_case("extras[155] under custom table", [0x0107, 0x0142], [5, 6, 4, 27, 5, 6])
extras_case("extras[157] past custom table", [0x0107, 0x0142], [5, 6, 4, 29, 0, 0])
extras_case("empty table undefines all", [], [5, 6, 4, 27, 0, 0])
extras_case("surrogate pair 155,156", [0xD83D, 0xDE00], [5, 6, 4, 27, 5, 6, 4, 28])
extras_case("lone high surrogate", [0xD83D, 0xDE00], [5, 6, 4, 27, 0, 0])

print("== abbreviations (table $60, strings from $130) ==")


def abbrev_image(version: int, chunk: bytes, entry0: list[int] | None = None) -> Memory:
    data = base_image(version)
    plant(data, 0x18, (0x0060).to_bytes(2, "big"))
    for number, target in {0: 0x130, 32: 0x134, 95: 0x138}.items():
        plant(data, 0x60 + 2 * number, (target // 2).to_bytes(2, "big"))
    plant(data, 0x130, pack(entry0 if entry0 is not None else a0("go")))
    plant(data, 0x134, pack(a0("hi")))
    plant(data, 0x138, pack(a0("ok")))
    plant(data, 0xC0, chunk)
    return memory_of(data)


for version, zchars, label, entry0 in [
    (3, [1, 0] + a0("x"), "bank1 entry0 then x", None),
    (3, [2, 0, 0], "bank2 entry0", None),
    (3, [3, 31, 0], "bank3 entry31", None),
    (2, [1, 0, 0], "v2 bank", None),
    (3, [1, 0, 0], "nested abbreviation", [1, 1, 0]),
    (3, [1, 0, 0], "abbreviation ends incomplete", [5, 6, 1]),
]:
    memory = abbrev_image(version, pack(zchars), entry0)
    try:
        text, _ = decode_string(memory, 0xC0)
        print(f"  {label}: v{version} {zchars} -> {ascii(text)}")
    except ZMachineTextError as e:
        print(f"  {label}: v{version} {zchars} -> ERROR {e}")

print("== encode_word cases ==")
for version in (1, 2, 3, 4, 5):
    for word in ("hello", "xyzzy", "Frobozz", "x", "it's", "a1b2", "toRVALD", "ab<cd"):
        print(f"  v{version} {word!r} -> {encode_word(version, word).hex()}")

print("== encode under custom rows (v5) ==")
data = base_image(5)
plant(data, 0x34, (0x0150).to_bytes(2, "big"))
plant(data, 0x150, table)
rows = alphabets(memory_of(data))
for word in ("zy", "z0A"):
    print(f"  {word!r} -> {encode_word(5, word, rows).hex()}")

print("== char_to_zscii ==")
print(f"  a-umlaut -> {char_to_zscii(chr(0xE4))}")
print(f"  backspace -> {char_to_zscii(chr(8))}")
print(f"  input key 130 -> {char_to_zscii(chr(130))}")
try:
    char_to_zscii(chr(0x2603))
except ZMachineTextError as e:
    print(f"  snowman -> ERROR {e}")

print("== default extras ==")
print("  " + " ".join(f"{ord(c):04X}" for c in DEFAULT_EXTRAS))
print(f"  count={len(DEFAULT_EXTRAS)}")
