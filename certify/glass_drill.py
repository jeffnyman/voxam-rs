"""The glass drills: real sessions on a ConPTY, judged as screens.

The painted terminal cannot be certified byte-for-byte against the
reference -- the escape streams differ by design, ratatui's diff
against blessed's rows -- so the drill judges what a player would
see instead: each scenario runs the real binary in a Windows
pseudo-console, captures the raw VT stream, replays it onto a
virtual screen, and asserts the landmarks that matter. The
dirty-screen renders replay the same stream over a screenful of
fake shell noise, which is how the milestone's one field failure
-- a missing backend clear letting the shell show through -- is
pinned so it can never return.

Keystrokes are sent only after the screen shows the prompt they
answer, so the drill never races the machine's boot the way a
fixed sleep would.
"""

import sys
import threading
import time

import pyte
from winpty import PtyProcess

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

COLUMNS = 100
ROWS = 30
PATIENCE = 25.0


class Session:
    """One live glass session: a spawned binary and its stream."""

    def __init__(self, command: str) -> None:
        self.process = PtyProcess.spawn(command, dimensions=(ROWS, COLUMNS))
        self.captured: list[str] = []
        self._reader = threading.Thread(target=self._drain, daemon=True)
        self._reader.start()

    def _drain(self) -> None:
        while True:
            try:
                chunk = self.process.read(65536)
            except (EOFError, OSError):
                return

            if chunk:
                self.captured.append(chunk)

    @property
    def stream(self) -> str:
        return "".join(self.captured)

    def screen(self, *, dirty: bool = False) -> list[str]:
        """The stream so far, rendered onto a virtual screen.

        With dirty, the screen is first filled with fake shell
        noise: a stream that fails to clear the glass shows the
        noise bleeding through.
        """

        held = pyte.Screen(COLUMNS, ROWS)
        feed = pyte.Stream(held)

        if dirty:
            for index in range(ROWS * 2):
                feed.feed(f"PS F:\\old\\shell\\line{index} > {'#' * 60}\r\n")

        feed.feed(self.stream)

        return [line.rstrip() for line in held.display]

    def await_text(self, marker: str, *, patience: float = PATIENCE) -> bool:
        """Wait until the rendered screen shows the marker."""

        deadline = time.time() + patience

        while time.time() < deadline:
            if any(marker in line for line in self.screen()):
                return True

            time.sleep(0.25)

        return False

    def type_line(self, text: str) -> None:
        self.process.write(text + "\r")

    def press(self, key: str = "\r") -> None:
        self.process.write(key)

    def close(self) -> None:
        if self.process.isalive():
            self.process.terminate(force=True)


FAILURES: list[str] = []


def press_until(session: Session, marker: str, *, presses: int = 15) -> bool:
    """Press SPACE through a story's keypress beats until a marker.

    Openings pause on unannounced any-key beats -- a cover, a
    title card, a paged prologue -- and each press is spent only
    when the marker has not yet painted, so no stray key leaks
    into the read that follows.
    """

    for _ in range(presses):
        if any(marker in line for line in session.screen()):
            return True

        session.press(" ")
        time.sleep(0.8)

    return session.await_text(marker, patience=5)


def judged(
    drill: str, verdict: bool, telling: str, session: "Session | None" = None
) -> None:
    if verdict:
        print(f"GLASS OK: {drill} -- {telling}")
    else:
        FAILURES.append(drill)
        print(f"GLASS PARTED: {drill} -- {telling}")

        if session is not None:
            for index, line in enumerate(session.screen()):
                if line:
                    print(f"  {index:2}|{line}")


def zork_drill(binary: str, corpus: str) -> None:
    """Zork I: status bar, echo, a clean wipe, a clean quit."""

    session = Session(f"{binary} --seed 1 {corpus}/zcode-infocom/zork1-r88-s840726.z3")

    try:
        if not session.await_text("There is a small mailbox here."):
            judged("zork", False, "the opening never painted", session)
            return

        session.type_line("quit")

        if not session.await_text("leave the game"):
            judged("zork", False, "quit never asked its question", session)
            return

        # Judged mid-read, while the glass still owns the screen:
        # the exit's own newline scrolls the top row away.
        screen = session.screen(dirty=True)

        judged(
            "zork",
            "West of House" in screen[0]
            and any(">quit" in line for line in screen)
            and not any("#" in line for line in screen),
            "status bar on the top row, commands echoed, the shell wiped",
            session,
        )
        session.type_line("y")
    finally:
        session.close()


def cover_note_drill(binary: str, corpus: str) -> None:
    """A JPEG cover's refusal stands on the story's first screen."""

    session = Session(
        f"{binary} --seed 1 {corpus}/zcode-infocom/zork1-r88-s840726.zblorb"
    )

    try:
        if not session.await_text("West of House"):
            judged("cover-note", False, "the story never painted", session)
            return

        screen = session.screen()

        judged(
            "cover-note",
            any("JPEG, which Voxam cannot draw" in line for line in screen),
            "the refusal note stands above the banner",
        )
    finally:
        session.close()


def scroll_thief_drill(binary: str, corpus: str) -> None:
    """A Glulx story takes the whole glass over a dirty screen."""

    session = Session(
        f"{binary} --seed 1 {corpus}/glulx-code/scroll-thief-r2-s150729.gblorb"
    )

    try:
        # The story opens on its own menu, and the introduction
        # behind it advances a keypress at a beat.
        if not session.await_text("Press [SPACE] to begin."):
            judged("scroll-thief", False, "the opening menu never painted", session)
            return

        if not press_until(session, "What do you want to write?"):
            judged("scroll-thief", False, "the name prompt never painted", session)
            return

        clean = not any("#" in line for line in session.screen(dirty=True))

        session.type_line("Drill")

        settled = session.await_text("addressed as")

        judged(
            "scroll-thief",
            clean and settled,
            "the glass cleared the shell and the typed line landed",
        )
    finally:
        session.close()


def amiga_colours_drill(binary: str, corpus: str) -> None:
    """Beyond Zork paints its palette under the amiga identity."""

    session = Session(
        f"{binary} --seed 1 --interpreter amiga "
        f"{corpus}/zcode-infocom/beyondzork-r57-s871221.z5"
    )

    try:
        # The half-block cover waits for a key, and the sidecar
        # Blorb's prologue pages behind [MORE] after it.
        time.sleep(2.0)

        if not press_until(session, "BEGIN, RESTORE or QUIT"):
            judged("amiga-colours", False, "the opening menu never painted", session)
            return

        # An empty line first spends any straggler the paging
        # left queued, so BEGIN arrives unprefixed; the menu
        # simply asks again.
        session.type_line("")
        time.sleep(1.0)
        session.type_line("begin")

        # BEGIN opens the prologue, which pages behind [MORE] on
        # its way to the setup screen.
        if not press_until(session, "Character Setup"):
            judged("amiga-colours", False, "the setup screen never painted", session)
            return

        # The classic dim palette: white ink (7) on black paper (0),
        # the shades blessed spells as SGR 37 and 40.
        judged(
            "amiga-colours",
            "[38;5;7" in session.stream and "48;5;0" in session.stream,
            "the amiga identity lights the dim palette",
        )
    finally:
        session.close()


def main() -> int:
    binary = sys.argv[1]
    corpus = sys.argv[2]

    zork_drill(binary, corpus)
    cover_note_drill(binary, corpus)
    scroll_thief_drill(binary, corpus)
    amiga_colours_drill(binary, corpus)

    if FAILURES:
        print(f"certify: {len(FAILURES)} glass drills parted: {', '.join(FAILURES)}")

        return 1

    print("certify: 4 glass drills, every screen as a player would see it")

    return 0


if __name__ == "__main__":
    sys.exit(main())
