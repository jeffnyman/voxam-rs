"""The reference half of the object table parity sweep.

Prints a story's object table in the same form the port's example
prints it: the census by the lowest-property-table heuristic, then
every object's family relations, raw attribute bytes, decoded short
name, and property walk.
"""

import sys
from pathlib import Path

from voxam.errors import VoxamError
from voxam.zmachine.memory import Memory
from voxam.zmachine.objects import ObjectTable
from voxam.zmachine.story import Story
from voxam.zmachine.zscii import decode_string

story = Story.load(Path(sys.argv[1]))
memory = Memory(story)
table = ObjectTable(memory)

version = story.version
v3 = version <= 3
base = memory.header.object_table_address
entries = base + 2 * (31 if v3 else 63)
entry_size = 9 if v3 else 14
attribute_bytes = 4 if v3 else 6
max_object = 255 if v3 else 65535

# The census: walk objects until an entry would overlap the lowest
# property table seen.
limit = None
count = 0

for obj in range(1, max_object + 1):
    offset = entries + (obj - 1) * entry_size

    if limit is not None and offset + entry_size > limit:
        break

    try:
        property_table = table.short_name_address(obj) - 1
    except VoxamError:
        break

    if property_table > 0:
        limit = property_table if limit is None else min(limit, property_table)

    count = obj

print(f"base={base:04X} count={count}")


def describe(obj: int) -> str:
    parent = table.parent(obj)
    sibling = table.sibling(obj)
    child = table.child(obj)

    entry = entries + (obj - 1) * entry_size
    attributes = "".join(
        f"{memory.read_byte(entry + offset):02X}" for offset in range(attribute_bytes)
    )

    name_address = table.short_name_address(obj)
    name_words = memory.read_byte(name_address - 1)

    if name_words == 0:
        name = "-"
    else:
        text, _ = decode_string(memory, name_address)
        name = " ".join(f"{ord(ch):04X}" for ch in text)

    # One linear pass down the property list (§12.4), capped at 64
    # entries: no legitimate list exceeds 63 properties, so the cap
    # only truncates walks through junk -- which a corrupt story can
    # offer -- identically on both sides.
    properties = ""
    address = name_address + 2 * name_words

    for step in range(65):
        if step == 64:
            properties += " ..."
            break

        first = memory.read_byte(address)

        if first == 0:
            break

        if v3:
            number, length, data = first & 0x1F, first // 32 + 1, address + 1
        elif first & 0x80:
            length = memory.read_byte(address + 1) & 0x3F
            number, length, data = first & 0x3F, length or 64, address + 2
        else:
            number, length, data = first & 0x3F, 2 if first & 0x40 else 1, address + 1

        payload = "".join(
            f"{memory.read_byte(data + offset):02X}" for offset in range(length)
        )
        properties += f" {number}:{payload}"
        address = data + length

    return f"p={parent} s={sibling} c={child} a={attributes} n=[{name}] props{properties}"


for obj in range(1, count + 1):
    try:
        print(f"{obj}: {describe(obj)}")
    except VoxamError as error:
        print(f"{obj}: ERROR {error}")
