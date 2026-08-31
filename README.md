# Voxam

_A Specification-Accurate Interactive Fiction Interpreter_

Voxam is an interactive fiction interpreter written in Rust,
speaking three virtual machines:

- **Z-Machine**, story file versions 1 through 8 -- the Infocom-era
  formats (v1-v6) and the later extensions that Inform emits (v7
  and v8), including the v6 windowed games.
- **Glulx**, Inform's 32-bit target, with a full Glk layer behind
  it.
- **The Å-machine**, Dialog's own target, played at the terminal
  with the LOOK chunk's styles worn as terminal attributes.

It is a port of [Voxam](https://github.com/jeffnyman/voxam), the
Python implementation, which remains the reference: every seeded
session recorded there replays byte-identically here, and the
parity sweeps in `certify/` prove it on demand. `PORT.md` maps the
porting territory -- what is done, what is next, and what each
milestone's gate was.

## Playing

Point the binary at a story file and type at it:

```sh
cargo run --release -- stories/zork1.z3
cargo run --release -- stories/advent.ulx
cargo run --release -- stories/cloak.aastory
```

The machine is chosen by the file's own magic, not its name.
Packaged stories (`.zblorb`, `.gblorb`) unwrap themselves, and a
bare story beside a `.blb` sidecar finds its resources. Saved
games are interchange-true: Quetzal for the Z-Machine (readable by
dfrotz and friends), IFZS for Glulx, and the Å-machine's AASV
files.

At a real terminal, stories play on the painted glass: the
Z-Machine's §8 windows cell for cell (Blorb cover art included,
shown Infocom's way -- the picture, a keypress, the story), the
Glk window tree for Glulx, and the Å-machine dressed in the
story's own styles. Piped -- or asked with `--plain` -- everything
keeps the plain stream: text out, lines in, end of input ends the
session.

A few flags:

```sh
# Describe a Z-Machine story's header (§11.1) and exit.
cargo run --release -- --header stories/zork1.z3

# Play with a fixed random seed, for reproducible sessions.
cargo run --release -- --seed 92 stories/zork1.z3

# Keep the plain stream even at a real terminal.
cargo run --release -- --plain stories/zork1.z3

# Claim a §11.1.3 interpreter identity. Games key real behavior
# off it: Beyond Zork paints its colors when it believes it is on
# an Amiga.
cargo run --release -- --interpreter amiga stories/beyondzork.z5

# Draw a Blorb's cover art in real sixel pixels rather than
# half-block cells -- for terminals that speak sixel, like
# Windows Terminal 1.22 and later.
cargo run --release -- --pixels stories/zork1.zblorb

# Replay a recorded acceptance script (Z-Machine and Glulx).
cargo run --release -- --accept acceptance/zork1-r88-s840726.accept

# Play in the browser: the vendored GlkOte display over HTTP.
cargo run --release -- --web stories/zork1.z3
cargo run --release -- --web --port 8931 stories/advent.ulx

# Speak the GlkOte protocol as JSON lines on stdin and stdout --
# the seam a display shell drives.
cargo run --release -- --glkote stories/cloak.aastory
```

All three machines speak the wire: `--web` serves the story at
http://127.0.0.1:8080 with the machine's own tab icon and, when a
Blorb's iFiction record names it, the story's own title; a page
reload restarts the game. `--glkote` is the same conversation on
stdio, one stanza per line, for shells that host the display
themselves. Version 6 Z-Machine stories play on the §8.8 stage:
one scaled canvas, the art's own coordinate space.

## The desktop shell

The Tauri shell lives in `desktop/`: the same GlkOte display in a
native window, with the interpreter linked into the shell itself.
The development loop:

```sh
cd desktop
npm install                    # once, for the tauri CLI
npm run tauri dev
```

The machines run on a thread of the shell's own process, speaking
the same GlkOte wire over an in-memory pipe -- so there is no
interpreter to find, none to bundle beside the installer, and a
player installs one thing and plays. The CLI's `--glkote` stays
what it always was: the certification wire the sweeps drive.

The **map** pane (View > Map, or Ctrl+M) draws the rooms you have
walked and the passages you took between them, following you as
you go and coming back where you left it the next time you play
that story. It is drawn from what the interpreter reports rather
than from reading the prose, so it only ever shows what actually
happened: passages are one-way until walked both ways, undo and
restore never invent a corridor, and a story that does not report
where the player is says so instead of drawing a fiction.

A map is kept between sittings, so a story picked up again opens
where its walking left off; View > Forget This Map throws one
away when a walk is better started over, asking first, and never
touching the notes.

The **notes** pane (View > Notes, or Ctrl+E) is a notepad per
story, saved as you write and kept under the same IFID, in plain
text you can open in anything. Its one button writes in the room
you are standing in. Both panes can stand open at once, sharing
the column.

Still to come -- the Tauri 2 mobile targets, and a wasm face for
the browser -- `PORT.md` holds the order they arrive in.

## Building

```sh
cargo build
cargo build --release
```

The tests and lints the project gates on:

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

The shell stands outside the workspace, so it gates on its own --
its map and sidecar reading are ordinary Rust with ordinary tests:

```sh
cd desktop/src-tauri
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

The deeper certification -- every sweep diffing this port against
the Python reference over real story files, recordings, and gold
transcripts -- lives in `certify/`, with its own README.

## Development

Commit messages follow the
[Conventional Commits](https://www.conventionalcommits.org/) specification,
enforced locally by [cocogitto](https://github.com/cocogitto/cocogitto).
The hook definitions live in `cog.toml`, but Git hooks themselves never
travel with a clone, so activating enforcement is a one-time step per
machine:

```sh
cargo install cocogitto
cog install-hook --all
```

After that, any commit whose message does not parse as a conventional
commit is rejected at commit time. To check a message without
committing:

```sh
cog verify "feat: add object table parsing"
```
