"""The reference's half of the Å-machine story sweep.

Prints the same report as the port's aastory example: the header's
claims, the bibliography, the extended character table, the chunk
census, and the whole dictionary decoded. Run from the reference
checkout with uv, as the sweep does.
"""

import sys
from pathlib import Path

from voxam.aamachine.story import Story
from voxam.aamachine.text import Speech
from voxam.errors import AAMachineError


def main() -> None:
    data = Path(sys.argv[1]).read_bytes()

    try:
        story = Story(data)
    except AAMachineError as error:
        print(f"REFUSED: {error}")
        return

    print(
        f"version={story.version[0]}.{story.version[1]} "
        f"wordsz={story.word_size} shift={story.shift} "
        f"release={story.release} serial={story.serial} "
        f"checksum={story.checksum:08x} "
        f"heap={story.heap_size} aux={story.aux_size} ram={story.ram_size}"
    )
    print(f"ifid={story.ifid if story.ifid is not None else '-'}")

    for name, value in story.meta.items():
        print(f"meta {name}={value}")

    print(f"extended={len(story.extended)}:{''.join(story.extended)}")

    census = ",".join(
        f"{held.chunk_id.decode('ascii', 'replace')}:{len(held.payload)}"
        for held in story.chunks
    )

    print(f"chunks={census}")
    print(f"files={len(story.files)}")

    try:
        speech = Speech(story)
    except AAMachineError as error:
        print(f"REFUSED: {error}")
        return

    print(f"words={len(speech.words)}")

    for seat, word in enumerate(speech.words):
        print(f"{seat}: {word}")


if __name__ == "__main__":
    main()
