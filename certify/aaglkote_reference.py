"""One Å-machine GlkOte session on stdio: the reference's serving.

Run from the reference checkout (uv run python .../aaglkote_reference.py
<story> [seed]): serves the protocol on stdin and stdout until the
display hangs up.
"""

import sys
from pathlib import Path

from voxam.aamachine.glkote import serve
from voxam.aamachine.story import Story


def main() -> int:
    story = Story(Path(sys.argv[1]).read_bytes())
    seed = int(sys.argv[2]) if len(sys.argv) > 2 else None

    return 0 if serve(story, sys.stdin, sys.stdout, seed=seed) else 1


if __name__ == "__main__":
    sys.exit(main())
