"""Drive one Z GlkOte session from an acceptance recording.

The wire twin of the replay sweep's typist: the recording's
commands become the display's events, sent one ask at a time. The
driver reads each update the session writes, prints it verbatim as
the transcript -- what the sweep diffs -- and answers the standing
ask from the script: a line read takes the next command, a
keystroke read spends the next command one character at a time, a
file ask gets a slot in the given directory, and a click marker
lands on the grid at its recorded cell. Timers are never fired, so
the drive stays deterministic. A script that is not an .accept
recording reads as plain input lines, verbatim -- the Å-machine
fixtures' own .in scripts.

The policy needs no fidelity to the blocking replay's own key
accounting: both implementations are driven with the same stream,
and identical streams earning identical transcripts is the whole
certification.

Usage (run under the reference's environment, which parses the
recording grammar):

    python zglkote_drive.py <recording.accept> <savedir> [--cwd DIR]
        -- <command> [args...]
"""

import json
import os
import subprocess
import sys
from pathlib import Path

from voxam.acceptance import CLICK, DOUBLE_CLICK, AcceptanceScript


def main() -> int:
    dash = sys.argv.index("--")
    recording = Path(sys.argv[1])
    savedir = Path(sys.argv[2])
    cwd = None
    extras = sys.argv[3:dash]

    if extras[:1] == ["--cwd"]:
        cwd = extras[1]

    if recording.suffix == ".accept":
        script = AcceptanceScript.parse(recording)
        told_commands = list(script.commands)
        told_clicks = list(script.clicks)
    else:
        told_commands = recording.read_text(encoding="utf-8").splitlines()
        told_clicks = []

    environment = {**os.environ, "PYTHONUTF8": "1"}
    child = subprocess.Popen(
        sys.argv[dash + 1 :],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        cwd=cwd,
        env=environment,
        text=True,
        encoding="utf-8",
    )

    assert child.stdin is not None and child.stdout is not None

    commands = told_commands
    clicks = told_clicks
    pending: list[str] = []
    generation = 0
    grid = None
    ask = None

    def send(stanza: dict) -> None:
        child.stdin.write(json.dumps(stanza) + "\n")
        child.stdin.flush()

    def next_line() -> str | None:
        """The next scripted line: buffered keystrokes first."""

        if pending:
            held = "".join(pending).rstrip("\n")

            pending.clear()

            return held

        return commands.pop(0) if commands else None

    def next_key() -> str | None:
        """One scripted keystroke, a command spent char by char."""

        if not pending:
            if not commands:
                return None

            pending.extend(commands.pop(0) or "\n")

        return pending.pop(0)

    send(
        {
            "type": "init",
            "gen": 0,
            "support": ["timer", "graphics", "graphicswin", "colors", "sound"],
            "metrics": {
                "width": 800,
                "height": 480,
                "gridcharwidth": 10,
                "gridcharheight": 20,
            },
        }
    )

    try:
        while True:
            line = child.stdout.readline()

            if not line:
                break

            print(line.rstrip("\n"))

            stanza = json.loads(line)
            kind = stanza.get("type")

            if kind == "error" or stanza.get("exit"):
                break

            if kind == "update":
                generation = stanza.get("gen", generation)

                for window in stanza.get("windows", []):
                    if window.get("type") == "grid":
                        grid = window.get("id")

                if "specialinput" in stanza:
                    send(
                        {
                            "type": "specialresponse",
                            "gen": generation,
                            "response": "fileref_prompt",
                            "value": (savedir / "slot.sav").as_posix(),
                        }
                    )

                    continue

                if "input" in stanza:
                    ask = next(
                        (
                            entry
                            for entry in stanza["input"]
                            if entry.get("type") in ("line", "char")
                        ),
                        None,
                    )

            if ask is None:
                break

            event = answered(ask, generation, grid, next_line, next_key, clicks)

            if event is None:
                break

            send(event)
    finally:
        child.stdin.close()

        for line in child.stdout:
            print(line.rstrip("\n"))

        child.wait()

    return 0


def answered(ask, generation, grid, next_line, next_key, clicks):
    """The event answering one standing ask, or None when the
    script is spent."""

    if ask.get("type") == "line":
        told = next_line()

        if told is None:
            return None

        if told and told[0] in (CLICK, DOUBLE_CLICK) and clicks and grid is not None:
            x, y = clicks.pop(0)

            return {
                "type": "mouse",
                "gen": generation,
                "window": grid,
                "x": x - 1,
                "y": y - 1,
            }

        return {
            "type": "line",
            "gen": generation,
            "window": ask.get("id"),
            "value": told,
        }

    key = next_key()

    if key is None:
        return None

    value = "return" if key == "\n" else key

    return {
        "type": "char",
        "gen": generation,
        "window": ask.get("id"),
        "value": value,
    }


if __name__ == "__main__":
    sys.exit(main())
