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
├── certify/              the parity sweeps and golden-vector oracles
└── entharion/            (submodule) specs and story files, as in voxam
```

The recordings stay in the reference checkout: the replay sweep
reads `../voxam/acceptance/` (or `VOXAM_REFERENCE`), because the
recordings are the contract *between* the implementations and the
reference keeps custody.

Planned additions, in the order they're earned:

- `crates/voxam-glass/` — the painted terminal (ratatui), if it
  outgrows the CLI crate.
- `desktop/` — the Tauri shell, carried over from voxam and taught
  to link `voxam-core` in-process instead of spawning a subprocess;
  later, the Tauri 2 mobile targets.
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
| `png.py`, `aiff.py`, `wav.py`, `sixel.py`, `font3.py` | `voxam-core` or drop | `png` crate or a straight port; the hand-rolled decoders exist for zero-dep purity, which crates.io relaxes. Decide per file. |
| `acceptance.py`, `regtest.py`, `probe.py` | `voxam-core::harness` (or a dev-only crate) | The certification machinery. Ports early — it is how everything else is judged. |
| `listing.py`, `glance.py`, `decompose.py`, `scribe.py` | `voxam` (CLI) | Inspection tools; `--listing` doubles as a decoder test. |
| `cli.py` (2.3k lines) | `voxam` (CLI) | Flag surface preserved; `clap` or hand-rolled. |
| `glass.py`, `painter.py`, `screen.py`, `frontend.py` | ratatui face | **Rewrite in kind, not a port.** blessed's cell painting maps to ratatui's buffer model, not line-for-line. |
| `stage.py`, `gallery.py`, `speaker.py` | deferred | The pygame window. The webview face already renders V6 art, sound, and mouse; decide later whether a native window still earns its keep. |
| `web.py`, `glkote.py`, `*/glkote.py` | `voxam` (CLI) | `--web` on a stdlib-adjacent server (`tiny_http` or hand-rolled), `--glkote` as serde_json over stdio. The wire types live in `voxam-core`. |
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
   (Babel identities remain, folded into a later milestone.)
3. **Glulx, plain.** The VM, strings, floats, accel, Glk over the
   plain stream. *Gate:* glulxercise says "All tests passed."
4. **Å-machine.** *Gate:* the reference batteries replay
   byte-identical, as they do under Python.
5. **The wire.** `--glkote` stanzas over stdio, `--web`, the
   `voxam:` sidecar block. *Gate:* the existing Tauri shell plays
   stories against the Rust binary, unmodified.
6. **The painted terminal.** ratatui glass for all three machines.
   *Gate:* filmstrip comparisons where determinism allows; eyes
   otherwise.
7. **The deluxe shell.** Interpreter linked in-process, automapper
   and notes as panes, keyed by IFID. Then Tauri mobile, then wasm.

Milestones 1–4 are mechanical translation with a safety net.
Milestone 6 is the largest rewrite. Milestone 7 is new work the
Python implementation never had.

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
- **One dependency so far**: `getrandom`, standing in for
  `os.urandom` — the per-file relaxation of the hand-rolled
  purity rule, as anticipated below.

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
