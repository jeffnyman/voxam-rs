"""The reference half of the stage sweep: ten drills through the
§8.8 StageModel, the grid, cursors, sweeps, paints, and [MORE]
pauses printed in a canonical spelling the port must match line
for line."""

from voxam.errors import ZMachineScreenError
from voxam.frontend import Status
from voxam.screen import BOLD, REVERSE
from voxam.stage import FillPaint, ShiftPaint, StageModel, TextPaint


def staged() -> StageModel:
    return StageModel(columns=20, lines=10, font_width=10, font_height=10)


def spelled(paint) -> str:
    if isinstance(paint, TextPaint):
        cell = paint.cell

        return (
            f"text {paint.line} {paint.column} {cell.character} "
            f"{cell.style} {cell.foreground} {cell.background} {cell.font}"
        )

    if isinstance(paint, FillPaint):
        return (
            f"fill {paint.line} {paint.column} {paint.height} "
            f"{paint.width} {paint.background}"
        )

    assert isinstance(paint, ShiftPaint)

    return (
        f"shift {paint.line} {paint.column} {paint.height} "
        f"{paint.width} {paint.rise}"
    )


def told(stage: StageModel, pauses: list) -> None:
    for row in range(1, stage.lines + 1):
        print(f"|{stage.row_text(row)}|")

    line, column = stage.get_cursor()
    screen_line, screen_column = stage.screen_cursor()

    print(
        f"cursor {line} {column} screen {screen_line} {screen_column} "
        f"selected {stage.selected} ink {stage.foreground} paper {stage.background}"
    )
    print("sweep", *stage.sweep())

    for paint in stage.paints():
        print(spelled(paint))

    for pause in pauses:
        print("more", *pause)

    pauses.clear()


def hung(stage: StageModel) -> list:
    pauses: list = []
    stage.more = lambda line, column, ink, paper: pauses.append(
        (line, column, ink, paper)
    )

    return pauses


def main() -> int:
    print("drill boot-wrap")
    stage = staged()
    stage.write("a stretch of words that wraps at the twentieth column")
    told(stage, [])

    print("drill scroll-at-bottom")
    stage = staged()
    stage.write("\n".join(str(n) for n in range(1, 14)))
    stage.write("\n\n14")
    told(stage, [])

    print("drill dressed-window")
    stage = staged()
    stage.place_window(3, 21, 51, 30, 80)
    stage.set_window(3)
    stage.set_style(REVERSE)
    stage.set_style(BOLD)
    stage.set_colour(3, 4)
    stage.set_font(4)
    stage.set_cursor(11, 21)
    stage.write("boxed words that run long enough to wrap twice inside")
    told(stage, [])

    print("drill split-dance")
    stage = staged()
    stage.write("\n\n\nfour")
    stage.split_window(20)
    stage.write(" deep")
    stage.set_window(1)
    stage.write("top of the strip")
    stage.set_window(0)
    stage.set_cursor(1, 1)
    stage.write("below")
    stage.split_window(45)
    stage.write("x")
    stage.split_window(100)
    stage.write("homed")
    told(stage, [])

    print("drill margins")
    stage = staged()
    stage.set_margins(0, 30, 50)
    stage.write("\n".join(str(n) for n in range(1, 12)))
    stage.write("\n12")
    stage.erase_line(25)
    stage.set_cursor(11, 111)
    stage.erase_line()
    stage.set_margins(0, 110, 110)
    stage.write("gone")
    told(stage, [])

    print("drill erases")
    stage = staged()
    stage.write("story text everywhere")
    stage.place_window(4, 11, 11, 20, 40)
    stage.set_window(4)
    stage.write("gone")
    print("erased", *stage.erase_window(4))
    print("erased", *stage.erase_window(-2))
    stage.split_window(30)
    print("erased", *stage.erase_window(-1))
    told(stage, [])

    print("drill scroll-window")
    stage = staged()
    stage.write("one\ntwo\nthree")
    stage.scroll_window(0, 10)
    stage.scroll_window(0, -10)
    stage.scroll_window(0, 25)
    told(stage, [])

    print("drill editing")
    stage = staged()
    stage.set_buffering(False)
    stage.write("abcdefghijklmnopqrstuvwx")
    stage.set_buffering(True)
    stage.write("yz" + " " * 17 + "end word")
    stage.rub_out()
    print("retreated", stage.retreat(3))
    stage.set_cursor(1, 171)
    stage.write_rectangle(["ab", "cd", "ef", "gh"])
    told(stage, [])

    print("drill more-budget")
    stage = staged()
    pauses = hung(stage)
    stage.set_colour(3, 4)
    stage.write("\n".join(str(n) for n in range(1, 11)))
    stage.rest()
    stage.write("\n" * 9)
    stage.set_line_count(0, -999)
    stage.write("\n" * 30)
    stage.set_line_count(0, 8)
    stage.write("\n")
    stage.place_window(0, 61, 1, 40, 200)
    stage.erase_window(0)
    stage.write("menu\n")
    told(stage, pauses)

    print("drill odd-metrics")
    stage = StageModel(columns=17, lines=7, font_width=7, font_height=9)
    stage.place_window(5, 8, 13, 40, 50)
    stage.set_window(5)
    stage.set_cursor(5, 11)
    stage.write("odd metrics land where")
    stage.place_window(7, 60, 110, 90, 90)
    stage.set_window(7)
    stage.write("edge overhang test")
    stage.erase_line(23)
    told(stage, [])

    print("drill refusals")
    stage = staged()

    for poke in (
        lambda: stage.erase_window(9),
        lambda: stage.set_window(8),
        lambda: stage.show_status(Status("Nowhere", 0, 0, time_game=False)),
    ):
        try:
            poke()
        except ZMachineScreenError as error:
            print("refused:", error)

    return 0


if __name__ == "__main__":
    import sys

    sys.exit(main())
