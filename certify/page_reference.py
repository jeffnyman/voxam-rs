"""The reference half of the GlkOte page sweep: the Page's exact stanza spellings."""

import json

from voxam.glkote import FLOWBREAK, Page

BOX = (0, 0, 640, 400)
TOP = (0, 0, 640, 30)


def compact(stanza):
    return json.dumps(stanza, separators=(",", ":"))


page = Page()
page.window(1, "buffer", 0, BOX)
print(compact(page.update()))

page.window(1, "buffer", 0, BOX)
page.buffer(1, [("normal", 3, "lånk\n"), FLOWBREAK, ("header", 0, "below")])
page.line_input(1, 80, initial="go", terminators=("escape", "func5"))
print(compact(page.update()))

page.window(1, "buffer", 0, BOX)
page.line_input(1, 80, initial="go", terminators=("escape", "func5"))
page.typed({1: "go nor"})
page.buffer(1, [("normal", 0, "clock\n")])
print(compact(page.update()))

grid = Page()
grid.window(1, "grid", 0, TOP, gridsize=(80, 2))
grid.window(2, "graphics", 0, BOX, graphsize=(320, 200), scaled=True)
grid.grid(1, [[("normal", 0, "Score 10   ")], []])
grid.draw(2, [{"special": "fill", "x": 0, "y": 0, "width": 8, "height": 8}])
grid.char_input(1, cursor=(3, 0))
grid.line_input(2, 40, cursor=(8, 184), cell=(8, 8), ink="#c0ffee")
grid.timer(100)
grid.prompt("write", "save")
print(compact(grid.update()))
