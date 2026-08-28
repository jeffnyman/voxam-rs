"""The reference half of the dictionary parity sweep.

Prints a story's dictionary in the same form the port's example
prints it: the table's shape, every entry decoded to hexadecimal
text units, and every thirteenth entry's decoded word looked back
up.
"""

import sys
from pathlib import Path

from voxam.errors import VoxamError
from voxam.zmachine.dictionary import Dictionary
from voxam.zmachine.memory import Memory
from voxam.zmachine.story import Story
from voxam.zmachine.zscii import decode_string

LOOKUP_SAMPLE_STRIDE = 13

story = Story.load(Path(sys.argv[1]))
memory = Memory(story)

base = memory.header.dictionary_address
separator_count = memory.read_byte(base)
entry_length = memory.read_byte(base + 1 + separator_count)
entries = base + 4 + separator_count

try:
    dictionary = Dictionary(memory)
except VoxamError as error:
    print(f"DICTIONARY ERROR {error}")
    sys.exit(0)

print(
    f"base={base:04X} separators={separator_count} "
    f"length={entry_length} count={dictionary.entry_count}"
)

for index in range(dictionary.entry_count):
    address = entries + index * entry_length

    try:
        text, _ = decode_string(memory, address)
        units = " ".join(f"{ord(ch):04X}" for ch in text)
        print(f"{index}: {units}")

        if index % LOOKUP_SAMPLE_STRIDE == 0:
            try:
                found = dictionary.lookup(text)
                print(f"  lookup -> {found:04X}")
            except VoxamError as error:
                print(f"  lookup -> ERROR {error}")
    except VoxamError as error:
        print(f"{index}: ERROR {error}")
