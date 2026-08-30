#!/bin/sh
# The glass drill sweep: real painted sessions on a ConPTY, judged
# as the screens a player would see -- milestone 6's filmstrip.
#
# The painted terminal cannot diff byte-for-byte against the
# reference (the escape dialects differ by design), so each drill
# runs the real binary in a pseudo-console, replays its VT stream
# onto a virtual screen, and asserts the landmarks: the status
# bar, the echoed commands, the cover note, the dirty-screen wipe
# that pins the milestone's one field failure, and Beyond Zork's
# palette under the amiga identity.
#
# Windows-only by nature (the glass's home ground): the drill
# rides a Windows pseudo-console through pywinpty. Elsewhere it
# reports unusable rather than pretending.
#
# Usage and contract as header-diff.sh: 0 identical in spirit --
# every drill green -- 1 parted, 2 unusable. The first run builds
# a small private uv environment under target/ for the drill's
# two libraries; UV_NATIVE_TLS rides along for networks that
# intercept TLS.

set -u

root=$(cd "$(dirname "$0")/.." && pwd)

case "$(uname -s)" in
MINGW* | MSYS* | CYGWIN* | Windows*) ;;
*)
    echo "certify: the glass drill needs a Windows pseudo-console" >&2
    exit 2
    ;;
esac

if ! command -v uv >/dev/null 2>&1; then
    echo "certify: the glass drill needs uv to stand its environment" >&2
    exit 2
fi

if ! cargo build --release --quiet --manifest-path "$root/Cargo.toml"; then
    echo "certify: the port does not build" >&2
    exit 2
fi

env_dir="$root/target/glass-drill-env"
python="$env_dir/Scripts/python.exe"

if [ ! -x "$python" ]; then
    UV_NATIVE_TLS=1 uv venv --quiet "$env_dir" || exit 2
    UV_NATIVE_TLS=1 uv pip install --quiet --python "$env_dir" pywinpty pyte || exit 2
fi

"$python" "$root/certify/glass_drill.py" \
    "$root/target/release/voxam.exe" \
    "$root/entharion"
