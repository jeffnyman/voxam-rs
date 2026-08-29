"""One Glulx GlkOte session on stdio: the reference's own serving.

Run from the reference checkout (uv run python .../gglkote_reference.py
<story> [seed]): loads the story and its resources exactly as the
CLI's --glkote path does, then serves the protocol on stdin and
stdout until the display hangs up.
"""

import sys
from pathlib import Path

from voxam.blorb import Blorb
from voxam.cli import BLORB_SUFFIXES, _glulx_resources
from voxam.glulx.glk.api import Glk
from voxam.glulx.glk.glkote import GlkOteFrontend, serve
from voxam.glulx.glk.resources import Resources
from voxam.glulx.machine import Machine
from voxam.glulx.story import Story


def main() -> int:
    path = Path(sys.argv[1])
    seed = int(sys.argv[2]) if len(sys.argv) > 2 else None

    if path.suffix.lower() in BLORB_SUFFIXES:
        blorb = Blorb.load(path)
        story = Story(blorb.glulx)
    else:
        blorb = _glulx_resources(path, None)
        story = Story(path.read_bytes())

    frontend = GlkOteFrontend()
    library = Glk(frontend, resources=Resources(blorb))
    machine = Machine(story, seed=seed, glk=library)

    return 0 if serve(machine, library, frontend, sys.stdin, sys.stdout) else 1


if __name__ == "__main__":
    sys.exit(main())
