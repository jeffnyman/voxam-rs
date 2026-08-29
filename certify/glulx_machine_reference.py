"""The machine-era oracle: the reference's answers for the bare
Glulx machine -- no Glk library installed.

Two acts, matching the port's glulx_machine example: the byte-exact
save of a known mutated state, then every .ulx in the corpus named
by argv[1] booted bare and run until it quits or halts.

Run from the reference checkout; certify's sweep arranges that.
"""

import sys
from pathlib import Path

from voxam.errors import VoxamError
from voxam.glulx import serial
from voxam.glulx.machine import Machine
from voxam.glulx.stack import DestType
from voxam.glulx.story import Story

IDLE = bytes([0xC0, 0x00, 0x00, 0x81, 0x20])

LIMIT = 200_000


def image() -> bytes:
    data = bytearray(0x200)
    data[0:4] = b"Glul"
    data[4:8] = (0x00030102).to_bytes(4, "big")
    data[8:12] = (0x100).to_bytes(4, "big")
    data[12:16] = (0x200).to_bytes(4, "big")
    data[16:20] = (0x300).to_bytes(4, "big")
    data[20:24] = (0x100).to_bytes(4, "big")
    data[24:28] = (0x48).to_bytes(4, "big")
    data[28:32] = (0x54).to_bytes(4, "big")
    data[0x48 : 0x48 + len(IDLE)] = IDLE

    checksum = sum(
        int.from_bytes(data[at : at + 4], "big") for at in range(0, len(data), 4)
    ) & 0xFFFFFFFF
    data[32:36] = checksum.to_bytes(4, "big")

    return bytes(data)


def save_vector() -> None:
    machine = Machine(Story(image()))

    machine.memory.write_byte(0x150, 0x42)
    machine.memory.set_size(0x400)
    machine.memory.write_byte(0x350, 0x77)
    machine.heap.alloc(0x40)
    machine.heap.alloc(0x30)
    machine.stack.push(123)
    machine.stack.push_stub(DestType.MEMORY, 0x140, 0x1234)

    print(f"save: {serial.serialize(machine).hex()}")


def bare_runs(corpus: Path) -> None:
    for path in sorted(corpus.glob("*.ulx")):
        try:
            machine = Machine(Story(path.read_bytes()))
        except VoxamError as error:
            print(f"{path.name}: boot refused: {error}")

            continue

        steps = 0

        try:
            while machine.running:
                if steps >= LIMIT:
                    print(f"{path.name}: still running after {steps} steps")

                    break

                machine.step()

                steps += 1
            else:
                print(f"{path.name}: quit after {steps} steps")
        except VoxamError as error:
            print(f"{path.name}: {steps} steps, halted: {error}")


save_vector()

if len(sys.argv) > 1:
    bare_runs(Path(sys.argv[1]))
