#!/bin/bash
# The disk usage count on a mount that is slow and on one that goes silent, through the real window and message loop.
# Uses scripts/slowfs.py (a FUSE file system that stands in for an SMB share). Needs a Wayland session, fusepy and
# libfuse 2. Exit status 2 means a part was missing and nothing was tested.
#   scripts/usage-netfs.sh
set -uo pipefail
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$root"

runtime=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
if [ -z "${WAYLAND_DISPLAY:-}" ] || [ ! -S "$runtime/$WAYLAND_DISPLAY" ]; then
    echo "no Wayland socket; not run" >&2
    exit 2
fi
venv=${FUSE_VENV:-/tmp/fusevenv}
if [ ! -x "$venv/bin/python" ]; then
    python3 -m venv "$venv" && "$venv/bin/pip" install --quiet fusepy || { echo "fusepy missing; not run" >&2; exit 2; }
fi
command -v fusermount >/dev/null || { echo "fusermount missing; not run" >&2; exit 2; }

cargo build --bin rust-commander 2>&1 | tail -1
work=$(mktemp -d /tmp/rc-netfs.XXXXXX)
mkdir -p "$work/src" "$work/mnt" "$work/home-slow" "$work/home-hang"
for i in $(seq 1 40); do
    mkdir -p "$work/src/d$i/s"
    for j in 1 2 3 4 5; do head -c 1000 /dev/zero > "$work/src/d$i/s/f$j"; done
done
cleanup() {
    fusermount -u "$work/mnt" 2>/dev/null
    [ -n "${fs_pid:-}" ] && kill "$fs_pid" 2>/dev/null
    rm -rf "$work"
}
trap cleanup EXIT

fail=0
for mode in slow hang; do
    extra=()
    [ "$mode" = hang ] && extra=(--hang-after 100 --hang-secs 45)
    "$venv/bin/python" scripts/slowfs.py "$work/src" "$work/mnt" --delay-ms 5 "${extra[@]}" >"$work/fs.log" 2>&1 &
    fs_pid=$!
    for _ in 1 2 3 4 5 6 7 8 9 10; do mount | grep -q "$work/mnt" && break; sleep 0.5; done
    mount | grep -q "$work/mnt" || { echo "could not mount slowfs: $(cat "$work/fs.log")" >&2; exit 2; }
    out=$(HOME="$work/home-$mode" XDG_CONFIG_HOME="$work/home-$mode/.config" XDG_DATA_HOME="$work/home-$mode/.local/share" \
        RUST_COMMANDER_AUDIO_DEVICE=null RUST_COMMANDER_SMOKE_NETFS="$work/mnt" RUST_COMMANDER_SMOKE_NETFS_MODE="$mode" \
        timeout 90 target/debug/rust-commander 2>&1 | grep SMOKE_NET)
    case "$out" in
        *SMOKE_NET_OK*) echo "ok    $mode: $out" ;;
        *) echo "FAIL  $mode: ${out:-no result}"; fail=1 ;;
    esac
    fusermount -u "$work/mnt" 2>/dev/null
    kill "$fs_pid" 2>/dev/null
    wait "$fs_pid" 2>/dev/null
    fs_pid=
done
exit "$fail"
