# The Port

Voxam-rs is a Rust port of [Voxam](https://github.com/jeffnyman/voxam),
the Python implementation at `../voxam`. This document maps the
territory: what goes where, in what order, and what certifies each
step. The Python implementation remains the reference throughout —
every behavior question is answered by reading it, and every seeded
session it records is a test this port must pass.

## Why Rust (recorded for posterity)

The core would port equally well to Go. Rust won on the frontends:
the desktop shell is already Tauri, Tauri 2 carries the same shell
to iOS and Android, the wasm target puts the interpreter in the
browser page itself, and the deluxe ambitions (automapper,
note-taker) want the machine linked in-process with the shell.
Exhaustive `match` over opcodes is also a fair trade for Python's
100%-branch gate: the compiler enforces what discipline used to.

## Workspace shape

```
voxam-rs/
├── Cargo.toml            workspace
├── crates/
│   ├── voxam-core/       the machines and formats; no I/O opinions
│   └── voxam/            the CLI binary: faces, flags, the wire
├── desktop/              the Tauri shell: the webview wearing GlkOte
├── certify/              the parity sweeps and golden-vector oracles
└── entharion/            (submodule) specs and story files, as in voxam
```

The recordings stay in the reference checkout: the replay sweep
reads `../voxam/acceptance/` (or `VOXAM_REFERENCE`), because the
recordings are the contract *between* the implementations and the
reference keeps custody.

The `desktop/` shell arrived with milestone 5: the reference's
Tauri shell carried over whole (its own standalone Cargo project,
kept out of the root workspace so the Tauri tree never rides
`cargo test --workspace`), spawning `voxam --glkote` as a child
and speaking to its stdio.

Milestone 7 swapped that subprocess for in-process linking, the
UI untouched: `src-tauri` now takes a path dependency on
`voxam-core` and serves each session on a thread of its own over
the linked host's pipes (the departure below). What went away
with the child: finding an interpreter beside the shell, the
missing-interpreter refusal, the console-window suppression, the
`--babel` subprocess a title bar cost, and the externalBin
bundling packaging would have owed -- an installer now carries
one executable, the shell with the machines inside it. The CLI's
`--glkote` stays exactly as it was: it is the certification wire,
and the sweeps drive it.

The `crates/voxam-glass/` crate arrived with milestone 6: the
painted terminal on ratatui -- the Z painter, the Glk display,
and the keystroke intakes -- kept out of `voxam-core` so the core
stays free of terminal opinions, and testable to the last cell
through ratatui's TestBackend. The CLI wires it in as the default
face at a real terminal, `--plain` keeping the stream.

Planned additions, in the order they're earned:

- the Tauri 2 mobile targets of `desktop/`.
- a `wasm` target of `voxam-core` married to glkote.js — the
  browser face with no server.

`voxam-core` stays free of display and filesystem opinions so the
same crate serves the CLI, the shell, and wasm. The seam between
core and face is the wire: GlkOte-shaped stanzas, extended with a
`voxam:` sidecar block (location object, printed name, score,
turns, the moving command) that the deluxe features consume.

## Module map

Python (`voxam/src/voxam/`) → Rust, mechanical unless noted.

| Python | Rust | Notes |
| --- | --- | --- |
| `zmachine/` (11.6k lines) | `voxam-core::zmachine` | Mechanical. Masking (`& 0xFFFF`) becomes native `u16`/`u32` wrapping. Dispatch dicts become `match`. |
| `zmachine/rng.py`, `glulx/rng.py` | `voxam-core::zmachine::rng`, … | **Port bit-exact first.** The xorshift32 is what makes every recording replay under the port. |
| `glulx/` (15k lines) | `voxam-core::glulx` | The Glk layer (`glk/`, ~7k) is the subtlest: object registries, dispatch. Floats via `f32::from_bits`. |
| `aamachine/` (5.5k lines) | `voxam-core::aamachine` | Mechanical; certified byte-identical against the reference batteries. |
| `iff.py`, `blorb.py`, `babel.py`, `infocom.py` | `voxam-core::{iff, blorb, babel, …}` | Byte work; IFIDs remain the persistence key for deluxe features. |
| `zmachine/quetzal.py`, `aamachine/saves.py` | with their machines | Interchange formats; saves must round-trip with other interpreters. |
| `png.py`, `aiff.py`, `wav.py`, `sixel.py`, `font3.py` | `voxam-core` or drop | Decided for `aiff`/`wav`/`png`: straight ports, zero-dep purity kept (the wire's data: urls need them, plus a hand-rolled base64 — and, for `png`, the hand-rolled `flate` module below). `sixel`/`font3`: decide when their era arrives. |
| `acceptance.py`, `regtest.py`, `probe.py` | `voxam-core::harness` (or a dev-only crate) | The certification machinery. Ports early — it is how everything else is judged. |
| `listing.py`, `glance.py`, `decompose.py`, `scribe.py` | `voxam` (CLI) | Inspection tools; `--listing` doubles as a decoder test. |
| `cli.py` (2.3k lines) | `voxam` (CLI), `voxam-core::session` | Flag surface preserved, hand-rolled. The story routing (`_play`'s Glulx-then-Å-then-Z order) lives in the core's session facade since milestone 7, bytes in, so every face begins identically. |
| `glass.py`, `painter.py`, `screen.py`, `frontend.py` | ratatui face | **Rewrite in kind, not a port.** Done for the terminal half with milestone 6: `screen.py` and `frontend.py` ported to `voxam-core`, `editor.py` beside them, and `painter.py` rewritten as `voxam-glass::painter` -- the model rendered whole into ratatui's buffer, whose diff replaces blessed's damaged-row repaints. The pygame `glass.py` window is not carried: its duties (Version 6 pictures, the arc band, the stage) are the desktop shell's, already served over the wire. |
| `glulx/glk/painted.py`, `glulx/glk/terminal.py`, `glulx/glk/wrap.py` | `voxam-glass::glk`, `voxam-core::glulx::glk::wrap` | The painted Glk spine and its terminal folded into one struct over ratatui's Backend seam; the wrapper ported whole. The pygame `glk/glass.py` stays with the shell, as above. |
| `gallery.py` | `voxam-core::gallery` | Ported with the deferred Blorb chunks (RelN, Reso, APal, BPal): sizes eager, pixels lazy, the adaptive-palette dance and the baked replacements whole; `Fraction` becomes the module's own exact `Ratio`. |
| `stage.py` | `voxam-core::stage` | The §8.8 model whole -- eight windows, one grid, unit paints, the [MORE] budget -- certified by the stage drill sweep. Python's `//` becomes `div_euclid`, so any negative unit floors identically. |
| `speaker.py` | deferred | The pygame window's voice. The webview face already plays V6 sound; decide later whether a native window still earns its keep. |
| `web.py`, `glkote.py`, `*/glkote.py` | `voxam` (CLI) | `--web` on a stdlib-adjacent server (`tiny_http` or hand-rolled), `--glkote` over stdio. The wire types live in `voxam-core`, on a hand-rolled JSON that keeps Python's exact spelling -- insertion-ordered keys, compact separators, ensure_ascii -- because the stanza sweeps diff byte for byte, which serde_json's dialect would part. |
| `filmstrip.py` | with the faces | Screenshot regression; wants the faces standing first. |
| `tests/` (42k lines) | selectively | Port unit tests for the decoders (ZSCII, operands, Quetzal, IFF); the recordings certify the rest end-to-end. |

## Milestones, each with its gate

1. **Plain-stream Z-machine.** ✅ *(2026-08-29)* Memory, ZSCII,
   objects, dictionary, opcodes, frames, the RNG — and past its
   own gate: the whole Version 1–8 opcode era, the §8.8 window
   ledger included. *Gate met:* every Z-code recording in the
   corpus — 42 of the 44, the other two being Glulx — replays
   byte-identically on the plain stream, refusal warnings and
   resource banners included.
2. **Persistence and packaging.** ✅ *(2026-08-29)* Quetzal
   save/restore, Blorb unwrapping and census, `--header` on
   packaged stories. *Gate met:* identical sessions write
   byte-identical saves under both implementations, restores
   cross both ways, and the saves travel to dfrotz and back.
   (The Babel identities landed with milestone 5's gate prep:
   the desktop shell titles its window by `--babel`.)
3. **Glulx, plain.** ✅ *(2026-08-29)* The VM, strings, floats,
   accel, Glk over the plain stream. *Gate met:* glulxercise says
   "All tests passed." — 596k instructions, 70 sections, zero
   failures — and both Glulx acceptance recordings replay
   byte-identically, which closes the ledger at 44 of 44.
4. **Å-machine.** ✅ *(2026-08-29)* The story file, the text
   apparatus, the engine with its Prolog heart, the AASV
   savefile, and the plain and terminal voices. *Gate met:* the
   reference batteries replay byte-identical -- the vendored gold
   transcripts (the community fork's own engine at seed 1234)
   land whole under the plain voice, Miss Gosling's Last Case
   included, and the terminal sessions diff clean against the
   reference implementation stream for stream.
5. **The wire.** ✅ *(2026-08-30)* `--glkote` stanzas over stdio
   for all three machines, `--web`, the Babel identities, and the
   §8.8 stage: Version 6 served as one scaled canvas, pictures
   through the adaptive-palette dance, the under-cursor samples
   minted. *Gate met:* the Tauri shell -- carried into this
   repository, everything running from here -- plays stories
   against the Rust binary, and every wire sweep runs
   byte-identical: zglkote 42 of 42 (Arthur, Journey, Shogun, and
   Zork Zero on the stage dialect included), gglkote 2 of 2,
   aaglkote 4 of 4 -- and the `voxam:` sidecar, designed into the
   reference first and ported in kind, rides every one of those
   sessions again under its own token-granted sweep, still
   byte-identical.
6. **The painted terminal.** ✅ *(2026-08-30)* ratatui glass
   for all three machines. *Gate met:* filmstrip comparisons
   where determinism allows, eyes otherwise -- both walked. The
   glass is built and wired -- the Z painter over the §8 screen model, the
   Glk display over the window tree, the line editor and both
   keystroke intakes, the painted face the CLI's default at a real
   terminal with `--plain` keeping the stream (the Å-machine's
   terminal voice, certified in milestone 4, is the third
   machine's). The deterministic half of the gate is met: the
   mirrored batteries hold every painting scenario to golden
   TestBackend grids, a corpus Zork I session serves end to end
   onto the test glass through the real loop, and every stanza and
   replay sweep still runs byte-identical around the new flag
   surface. The other half was eyes at a live
   terminal -- Beyond Zork's font-3 map and colours, Border
   Zone's ticking reads, Scroll Thief at the Glk glass -- and
   they earned their keep: the first eyes pass caught what the batteries
   could not and each finding closed the same day: the Glk glass
   was missing its backend clear -- painting blanks onto ratatui's
   already-blank model emits nothing, so the shell showed through
   every unpainted cell, which is why a Glulx story looked like
   text scattered over old prompts -- and a cover note printed to
   stdout was wiped the instant the glass cleared, so it now
   writes through the screen model and stands on the story's first
   screen. Both are pinned by the glass drill sweep
   (`certify/glass-diff.sh`): real sessions in a Windows
   pseudo-console, the VT stream replayed onto a virtual screen
   and judged as a player would see it, dirty-screen scenario
   included -- the milestone's filmstrip, arrived at last and
   kept. The eyes pass
   also asked after Beyond Zork's colours: with the default
   IBM-PC identity the reference painter is exactly as monochrome
   through the same screens, and the real key is the §11.1.3
   interpreter number -- Beyond Zork paints its palette when it
   believes it is on an Amiga. The `--interpreter` and `--tandy`
   flags now reach the play path as they always did in the
   reference, and the harness shows both implementations lighting
   the same colours under `--interpreter amiga` (the painter
   spells the classic dim SGR family, blessed's own shades). The sixel cover road shipped with
   the pass: `--pixels` draws a Blorb cover in real pixels on a
   terminal that speaks sixel (Windows Terminal 1.22+). The
   pass's deepest find was Border Zone: the two implementations
   proved identical to the game-second -- same ticks, same
   clocks, same screens, fast-forwarded side by side -- up to the
   planned scenes that read input from inside the clock
   interrupt, which the port then learned as the interrupt-frame
   departure below; the espionage now plays whole, trench-coat
   man and all, pinned by its own drill. Deferred
   within the milestone, each honestly claimed away: the speaker
   (no sound at the glass yet), terminal mouse reporting, the
   recording seams, and sixel *detection* -- the reference asks
   the terminal first and falls back on silence, but crossterm's
   parser consumes the device-attributes answer with no seam to
   read it, so until one exists the flag is an explicit opt-in,
   believed as asked and never the default.
7. **The deluxe shell.** Built, and awaiting its eyes pass. The
   session facade gathered the wire's beginning into
   `voxam-core::session` (and closed a Blorb divergence on the
   way); the shell stopped spawning `voxam --glkote` and now
   serves each session on a thread of its own over
   `voxam-core::pipe`, the page none the wiser; the sidecar is
   granted and read host-side; and the automapper and the
   notepad stand as panes, both filed under the story's IFID.
   *Gate:* the batteries of both trees, the linked sweep holding
   every Z recording identical across the two transports, and an
   eyes pass at the real window -- the one thing no headless
   check can stand in for.
8. **The mobile shell.** Tauri 2 carries `desktop/` to iOS and
   Android; the panes earn their keep on a small screen or learn
   another shape.
9. **The browser face.** A wasm target of `voxam-core` married to
   glkote.js -- the display with no server behind it.

Milestones 1–4 are mechanical translation with a safety net.
Milestone 6 is the largest rewrite. Milestone 7 onward is new
work the Python implementation never had.

## What the sidecar carries

The `voxam:` block rides each GlkOte update stanza, and it is the
whole of the interpreter's contribution to the deluxe features:
a dumb factual feed, with every ounce of graph, layout, and
rendering intelligence living in the face. Surveying prior art
confirmed that this small tuple is sufficient for a working
automapper, and taught one addition. The schema, designed into
the reference with milestone 5 and granted by the display's own
`"voxam"` init-support token (an ungranted session carries no
block at all):

- **The location**: object id and printed name. Per-machine
  honesty rules here — the Z-machine's global 0 is guaranteed
  only through v3 and conventional after, and Glulx has no fixed
  location global at all — so the fields are optional and honest
  rather than uniformly pretended, and the mapper degrades
  gracefully when location is unknowable.
- **The moving command, as delivered.** The wire layer knows the
  line it handed the machine — scripted input included — which
  beats any face-side memory of what was typed.
- **Score and turns**, as already planned.
- **A discontinuity flag**: this update does not follow causally
  from the last command. The machines all know when an undo,
  restore, restart, or death intervened; one honest bit here
  spares every face the transcript-grepping heuristics earlier
  automappers needed, and the mapper never draws a phantom edge
  across time travel.

The desktop shell granted the token with milestone 7 (its own
vendored `glkote.js` asks for `voxam` beside `stage` and the other
dialect words), and reads the block **in its own Rust**, in the
pump, before the stanza ever reaches the page: the deluxe
features' intelligence lives on the host side, and the webview is
left to wear the display alone. `sidecar::Bearings` is that
reading -- every field optional, a half-written location refused
rather than half-believed -- and its battery is pinned by blocks
captured from live Zork I sessions, never written by hand. The
browser face stays ungranted: it has no panes to feed, and an
ungranted session carries no block at all.

And a boundary: no direction-parsing or graph state in
`voxam-core`. Reading a typed command for its compass word is an
English-only, typed-input-only heuristic — fine as a face's
choice, poisonous as a core assumption. Persisted map layouts key
by IFID, per milestone 7's standing plan, which is part of why
the Babel identities work eventually matters.

## The map the shell draws

The automapper is the first work here with no reference to port
from, so its reasoning is written down rather than inherited. It
lives in the shell's own Rust (`desktop/src-tauri/src/map.rs`),
which is where the boundary above puts it: reading a typed command
for its compass word is an English-only, typed-input-only
heuristic, fine as a face's choice and poisonous as a core
assumption. The webview only draws what this module decides.

The rules, each earned:

- **Rooms are keyed by the location object**, so a room is never
  drawn twice and a renamed one (a dark room lit) keeps its place
  under its newest name. **Passages are directed**: one-way doors
  are ordinary in this medium, so walking north from A to B says
  nothing about walking south from B, and the reciprocal edge is
  drawn only when it is walked. **A placed room never moves** --
  the map is watched while it grows, and a layout that reshuffles
  under the player's eye is worse than a crooked corridor.
- **Up, down, in and out are marked edges on one plane**, not
  floors to page between: which floor a room belongs to is often
  unanswerable, and a marked edge never has to answer it.
- **Only what was walked is drawn.** The sidecar carries no exit
  list, and inferring untaken exits would mean reading room
  descriptions as prose -- the heuristic the boundary forbids.
- **A discontinuity draws nothing.** The interpreter's own bit
  says an undo, restore, restart, or death intervened, which
  spares the map every transcript-grepping guess earlier
  automappers needed.

Three of the rules exist because the corpus said so, not because
they were foreseen. The recordings were replayed through the
mapper itself (`mapwalk`, the shell's own instrument, fed by the
wire sweeps' driver under `VOXAM_SIDECAR=1`), and each finding
changed the design:

- **A line may hold several commands.** The house parsers all
  take `d. s. e`, the recordings lean on it heavily, and the wire
  reports one update at the *end* of the run. Drawing an edge
  across it claimed an adjacency that does not exist -- 48 of
  Zork's 64 passages were fiction on the first pass. A chain now
  draws no passage at all; when every leg was a compass word the
  destination is still *placed* by the summed vector, which beats
  a bare spiral. Zork's map fell to 20 passages, every one real.
- **Ships have their own compass.** `fore`, `aft`, `port`, and
  `sb` are directions wherever a game gives the player a vessel;
  they are most of Hitchhiker's movement, and laying them on the
  compass (consistently, making no claim about geography) took
  its readable passages from 12 to 23.
- **Some stories keep no location at all.** The location global
  is guaranteed only through Version 3 and conventional after
  (§8), and Adventure's Version 5 build reports one unchanging
  object named `Ob.ect` however far the player walks. A map cannot
  tell a wrong number from a right one, but it can notice that a
  dozen direction commands in a row moved the player nowhere --
  chains counted too, since that recording travels in them -- and
  say plainly that this story does not report where the player is.
  That is better than drawing one fictional room forever.

Persistence is one JSON file per IFID beside the display
settings, written only when the map actually grew, so a session
spent examining the scenery rewrites nothing. Since a map outlives
the sitting that drew it, there is a way to throw one away --
View > Forget This Map, asked before it is done, this story's map
alone, the notes untouched. Forgetting takes a wholly fresh map
rather than emptying the old one: a cell still claimed or a story
still disbelieved would haunt the walk that follows, which is
what the forgetting drill pins.

The pane (`desktop/ui/voxam-map.js`) only draws: it is handed
rooms with their cells, directed passages, and the room the
player stands in, and turns them into SVG. Its conventions follow
the model's decisions -- a compass passage is a plain line
because the model already placed those cells by that direction; a
vertical or in/out passage is dashed and lettered rather than
faking a bearing; an unnamed one is dotted; and a one-way passage
carries an arrowhead, so the arrows that appear are worth
noticing. One convention is the pane's own: because a placed room
never moves, a passage whose far room was placed elsewhere would
otherwise be ruled straight through every room between, so a
passage spanning more than one cell is bowed and faded instead.
The shape the pane reads -- `step.kind`, the lowercase way,
`here`, a room's `x` and `y` -- is pinned by a contract test on
the Rust side, so the two halves cannot drift apart silently.

The notes pane is the map's plainer neighbour: free text per
story, filed under the same IFID, and written as plain `.txt`
rather than anything of this shell's own devising, because a
player's notes should outlive the program that took them. It
saves a breath after typing stops rather than on every keystroke,
and settles whatever is pending when the box loses focus or the
window goes away, so a closing shell never takes the last
sentence with it. Its one button stamps in the room the player is
standing in, which is the note a player is usually about to write
anyway -- the only place the two panes touch. A story the Treaty
cannot name keeps its notes for the session only, since writing
them under no name would file one story's notes where another's
belong.

## Departures the port has recorded

Each is a documented translation, never a behaviour change; the
sweeps prove the outputs identical across all of them.

- **Suspension is a return value, not an exception.** Python
  raises through the step loop; here `step` returns
  `Step::Suspended` and `run` hands back `RunState::Waiting`,
  with the same parked-tail contract.
- **Text flows as 16-bit units.** Rust strings cannot hold lone
  UTF-16 surrogates, so decoding produces units and the fusing
  happens at String boundaries — the reference's exact
  fuse-after-decode composition.
- **State views take their stores as arguments.** ObjectTable,
  Variables, and the header's declare functions are geometry over
  `&Memory`/`&mut Memory` rather than owners, for the borrow
  checker's sake.
- **Opcode tables are matches with version guards** rather than
  dicts of span tuples; a §14 fork reads as two guarded arms.
- **Glk objects live in id-keyed arenas.** The reference's object
  graph — a window holding its parent pair, a stream its window —
  becomes maps keyed by internal id, with references stored as ids
  and the tree walks (`rearrange`, `subtree`) taking the map as an
  argument. Subclass hierarchies become `WindowKind`/`StreamKind`
  enums. The 32-bit ids Glulx sees stay the bridge's separate,
  lazily-minted sequence, exactly as in the reference, so
  transcripts diff identically.
- **Glk buffers are VM coordinates, not live views.** The
  reference's Buffer protocol (a list, or a MemArray over VM
  memory) becomes `MemArray` coordinates with `&Memory`/`&mut
  Memory` passed to every operation that touches one — the
  state-view departure applied to Glk. Retained arrays survive
  `setmemsize` for the same reason the reference's lazy views do.
- **The wire face is a shared cell, its machine halves on a
  Session.** The reference's `zmachine/glkote.py` face holds its
  machine and pokes it directly — a cycle Rust refuses. The face's
  state lives behind `Rc<RefCell<…>>`: the machine owns one handle
  as its `Frontend`, and a `Session` holds the other beside the
  machine itself, carrying the two halves that need both ends
  (`render`, `accept`). Two smaller spellings ride along: the
  timer-restart check compares the machine's new *wait serial*
  where the reference compares wait identity, and the arc band's
  header re-base is asked *by the machine* after each arc op
  (`arc_rows_below` on the trait) where the reference's face
  writes memory itself — the same bytes, the borrow's way around.
  The zglkote sweep proves the transcripts identical. The Glulx
  face repeats the pattern with two twists: its `render`/`accept`
  take the library and memory as arguments (the library owns the
  face's other handle), a file answer travels up as an `Accepted`
  verdict for the machine bridge's parked call, and the measured
  cells live in a small shared `Claims` cell of their own, because
  the library re-lays its tree mid-accept and asks the frontend
  for metrics while the face is borrowed. The Glk `Frontend` trait
  widened for it, each a recorded-departure spelling: the drawing
  and flow calls take the window map, `draw_image` the stream's
  link value, and the sound calls the channel's arena key (a
  snapshot carries no identity) plus the resources where a play
  can start. The Å-machine's face needed none of it: its machine
  owns the voice as a public field (`Machine<WireVoice>`), so the
  face holds no voice at all and takes it as an argument.
- **zlib is spelled by hand, on both sides of the mirror.** The
  reference leans on Python's stdlib `zlib` for PNG work; Rust's
  stdlib has none, so `flate.rs` hand-rolls the checksums, a full
  RFC 1951 inflate, and the deflate. The deflate is the deeper
  story: `zlib.compress`'s bytes are the backing library's own
  business — CPython 3.14 on Windows ships zlib-ng, which
  compresses differently from madler zlib — so the same reference
  emits different wire bytes on different machines, and certified
  bytes cannot. The fix went into the *reference first* (the
  standing not-frozen decision below): `png.py`'s `encoded()` now
  spells its own deterministic stream — one fixed-Huffman block,
  greedy matches through a last-seen table — and `flate.rs` ports
  it move for move, golden vectors pinning the bytes in both
  batteries. Three smaller spellings ride along: inflate refusal
  prose is the port's own words (the reference hears whatever its
  zlib build says), a PLTE chunk whose length is not a multiple of
  three drops the remainder where the reference's `iter_unpack`
  would crash, and `aamachine::story::crc32` moved to `flate` with
  a re-export keeping its old address.
- **The stage face is the same struct wearing a stage half.**
  The reference's StageFrontend subclasses its two-window face;
  here `GlkOteFrontend` carries `stage: Option<StageHalf>` and
  every seam that differs branches on it -- the subclass override,
  the borrow's way around. The machine's stage seams ride the
  ledger in units: the window ledger, the header's screen-units
  and font words, split_window's tiling, and stream 3's width
  arithmetic all consult one `unit_metrics` (8-by-8 on a measuring
  Version 6 glass, 1-by-1 everywhere else) -- Arthur right-aligns
  its ribbon from the $30 width word, which is how a cells-for-
  units slip was found. The zglkote sweep proves all four stage
  sessions identical.
- **The sidecar's machine handle is the serving loop's.** The
  reference's Glulx and Å-machine faces hold a machine attribute
  the loop attaches, and their sidecar blocks read the
  discontinuity bit through it; the Rust faces cannot hold their
  machines, so `sidecar(&mut machine.discontinuity)` is composed
  by the serving loop and handed into `render_with` -- the
  reference's default `voxam` argument, spelled as a delegating
  pair. The Z face needs none of it: render lives on the Session,
  which holds both ends. The sidecar sweep proves every granted
  telling identical.
- **The gallery caches behind `Rc`.** The reference's decode cache
  hands back the same object so re-plots are free; here `picture()`
  answers `Rc<Picture>` clones, `Rc::ptr_eq` standing in for the
  battery's `is_same_as`. Its `Fraction` becomes `gallery::Ratio`,
  an exact reduced i64 pair, so the Elbow Room arithmetic can never
  drift into floating point.
- **The painter renders whole and lets the buffer diff.** The
  reference repaints the rows the screen model reports damaged;
  the ratatui rewrite renders the entire grid every repaint and
  the library's double-buffer diff finds the changed cells -- the
  same minimal writes, the buffer's way around. The model's
  damage ledger is drained but no longer steers, and the golden
  assertions moved with the design: the mirrored batteries read
  painted TestBackend grids instead of escape streams. Two seams
  widened for the borrow: the model's [MORE] callback takes the
  model back (lifted out for the call and restored), and the
  editor's repaint takes its canvas back, since the canvas is
  lent to the read loop for the whole line.
- **The glass serving loop is the blocking path inside out.** The
  reference's painted frontends block inside the machine's read
  instructions; here the machine always suspends (the standing
  suspension departure), so read_line, read_key, the §15 timed
  ticking, the redisplay courtesy, and the abandon-on-terminate
  drill all live in the CLI's serving loop between `run` calls,
  delivered through `deliver_line`/`deliver_key`/`deliver_tick`.
  The face counts its prints so the loop can honour §15's
  redisplay remark the way the reference's machine counts its
  own. Saves never park: the glass face does not suspend, so the
  slot beside the story serves them, exactly the reference's
  painted arrangement.
- **The Glk display folds its spine and its terminal into one.**
  The reference splits the painted Glk display into a
  display-independent spine and a thin blessed terminal; the
  rewrite is one struct over ratatui's Backend seam, painting
  onto a persistent cell canvas -- "painting over is all the
  erasing there is" -- that rides the buffer diff each frame. The
  reference's posted-event back-reference is the standing
  `Asked::Instead` departure; its identity-keyed buffer texts
  become id-keyed wrappers pruned to the live tree each flush;
  its monkeypatchable clock becomes the display's own injectable
  one; and `prompt_file` widened to take the window map -- the
  tree-walk reshaping -- so the interrupted layout repaints once
  the prompt is answered.
- **Interrupt routines that read park in frames.** The §15
  timed-read interrupt is the suspension departure's hardest
  case: Border Zone's planned scenes print -- and sometimes
  prompt -- from inside the clock routine, which the reference
  handles by blocking recursion. Here a delivered tick runs the
  routine on a frame ledger: if a read suspends inside it, the
  routine's stack stays put, the inner read parks as the
  machine's wait for the host to serve like any other, and the
  outer read is held aside in the frame until the routine
  unwinds -- then its verdict restores or abandons the outer
  read, exactly as §15 asks. The glass serving loop follows by
  wait serial: a tick that parks a new read hands it back to the
  loop fresh, and the §15 redisplay courtesy returns the
  composed line below the scene once it is done. The trench-coat
  drill holds the whole nesting to Border Zone itself.
- **The wire's beginning is held once, in the core.** The
  reference's CLI routes every face to the machine that owns the
  story inside `_play`; the port had grown that routing three
  times over -- the play path, `--glkote`, and `--web` -- and the
  wire pair had drifted from the reference's order, refusing a
  Glulx Blorb as Z-code. Milestone 7's first move gathered the
  recognition and the GlkOte serving into `voxam-core::session`
  (`Opening`): bytes and a name in, never a path, so the same
  facade serves the CLI, the browser face, the desktop shell's
  in-process linking, and one day the wasm face, while the
  filesystem's share -- reading the story, finding a like-named
  sidecar -- stays with each caller. Re-walking the wire faces
  through the reference's own order closed the Blorb divergence:
  a `.gblorb` now serves over `--glkote` and `--web`, its opening
  stanza proven byte-identical against the reference, and the
  wire sweeps hold everything else exactly where it was.
- **The shell links the interpreter and pipes it by hand.** The
  reference's shell spawns `voxam --glkote` and speaks to the
  child's stdio; milestone 7 serves the session on a thread of the
  shell's own process instead, over a byte pipe spelled in
  `voxam-core::pipe` -- a shared queue with a condvar, blocking
  reads, and a hangup at each end, since the standard library
  offers no in-memory pipe and the purity rule argues against a
  crate for sixty lines. The page never learned of it: the
  `stanza`/`fault`/`ended` events keep their shapes and their
  session-id filtering, so `shell.js` did not change a line.
  Closing stdin became dropping the sender, killing the child
  became the same drop, the stderr drain became `catch_unwind`
  around the serving thread, and the `--babel` subprocess a title
  bar cost became a direct call. The machines are full of `Rc`
  handles and cannot cross a thread, so the story crosses as
  *bytes* and is opened over there -- which is what the facade's
  bytes-in shape was for. The linked sweep drives every Z
  recording through both transports and diffs them, so the
  hand-rolled pipe inherits the wire sweep's certification.
  **Its one honest cost:** a thread cannot be killed. A session
  standing at a read ends the moment its pipe hangs up, but one
  spinning inside a story (Dead Cities does exactly this, in both
  implementations) plays on unheard until the shell exits, where
  a child process could simply be killed. The road out is a
  cooperative stop flag the step loops consult -- the same
  mechanism the glass wants for its own Control-C escape when a
  story spins -- and it is deferred, named, until a spinning story
  is worth answering.
- **Control-C dies the reference's death by hand.** blessed's
  cbreak leaves SIGINT alive, so the reference session ends on
  the keypress; crossterm's raw mode eats it, so the intakes
  translate the chord themselves -- restore the shell, exit 130,
  the interrupted ending every shell recognises.
- **The dependency ledger**: `getrandom`, standing in for
  `os.urandom` — the per-file relaxation of the hand-rolled
  purity rule, as anticipated below — and, with milestone 6,
  `ratatui` (with its crossterm) in `voxam-glass` alone: the
  painted terminal is the one place the plan always named a
  library for, and `voxam-core` still carries none of it.

## Standing decisions

- **The RNG is sacred.** xorshift32 with the exact mixing
  constants, before anything that consumes it. A recording that
  replays under Python must replay here, forever.
- **entharion sits at the repo root**, as in voxam, so carried-over
  acceptance scripts keep their relative `GAME` paths.
- **Spec citations carry over.** The `§` references survive the
  translation; they are the project's conscience.
- **The Python implementation is not frozen** — it remains the
  reference and the place behavior questions get settled. The port
  chases it; recordings are the contract between them.
