"""The reference's half of the Å-machine terminal sweep.

Plays a story through the terminal face with the same scripted
streams as the port's aaterminal example: input lines from a
script file, filename prompts always cancelling, the session
undressed at width 80, seed 7. Run from the reference checkout
with uv, as the sweep does.
"""

import io
import sys
from pathlib import Path

from voxam.aamachine.story import Story
from voxam.aamachine.terminal import played


def main() -> None:
    story = Story(Path(sys.argv[1]).read_bytes())
    script = Path(sys.argv[2]).read_text(encoding="utf-8") if len(sys.argv) > 2 else ""
    writer = io.StringIO()

    played(
        story,
        seed=7,
        reader=io.StringIO(script),
        writer=writer,
        asked=lambda _prompt: "",
        width=80,
        dressed=False,
    )

    sys.stdout.write(writer.getvalue())


if __name__ == "__main__":
    main()
