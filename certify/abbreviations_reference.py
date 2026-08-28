"""The reference half of the abbreviation parity sweep.

Prints every abbreviation a story defines, one per line, as
space-separated hexadecimal text units -- before any surrogate
fusing, exactly the units the port's decoder produces.
"""

import sys
from pathlib import Path

from voxam.errors import VoxamError
from voxam.zmachine.memory import Memory
from voxam.zmachine.story import Story
from voxam.zmachine.zscii import decode_string

story = Story.load(Path(sys.argv[1]))
memory = Memory(story)
version = story.version
table = memory.header.abbreviations_table_address

# Version 1 has no abbreviations; Version 2 has one bank of 32,
# Version 3 up three banks of 96 (§3.3); a zero table address means
# none are defined.
count = 0 if version == 1 else 32 if version == 2 else 96

for entry_number in range(count if table else 0):
    try:
        entry = memory.fetch_word(table + 2 * entry_number)
        text, _ = decode_string(memory, 2 * entry)
        units = " ".join(f"{ord(ch):04X}" for ch in text)
        print(f"{entry_number}: {units}")
    except VoxamError as error:
        print(f"{entry_number}: ERROR {error}")
