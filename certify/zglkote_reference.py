"""One Z GlkOte session on stdio: the reference's own serving.

Run from the reference checkout (uv run python .../zglkote_reference.py
<story> [seed]): loads the story and its resources exactly as the
CLI's --glkote path does, then serves the protocol on stdin and
stdout until the display hangs up.
"""

import sys
from pathlib import Path

from voxam.cli import _load_story, _serve_z_glkote


def main() -> int:
    story, blorb = _load_story(Path(sys.argv[1]), None)
    seed = int(sys.argv[2]) if len(sys.argv) > 2 else None

    return _serve_z_glkote(story, blorb, seed=seed)


if __name__ == "__main__":
    sys.exit(main())
