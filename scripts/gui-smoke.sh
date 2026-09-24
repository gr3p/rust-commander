#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$root"

if ! command -v bwrap >/dev/null 2>&1; then
    echo "bubblewrap is missing; GUI smoke was not run" >&2
    exit 1
fi

runtime=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
socket=""
if [ -n "${WAYLAND_DISPLAY:-}" ] && [ -S "$runtime/$WAYLAND_DISPLAY" ]; then
    socket="$runtime/$WAYLAND_DISPLAY"
fi
if [ -z "$socket" ]; then
    echo "no Wayland socket; GUI smoke was not run" >&2
    exit 2
fi

if pkg-config --exists gtk4; then
    :
else
    prefix="$HOME/.local/opt/gtk4-prefix"
    if [ -d "$prefix/usr/lib/x86_64-linux-gnu/pkgconfig" ]; then
        export PKG_CONFIG_PATH="$prefix/usr/lib/x86_64-linux-gnu/pkgconfig:$prefix/usr/lib/pkgconfig:$prefix/usr/share/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
    fi
fi

cargo build --offline --bin comarchy

real_home=$HOME
set -- \
    --unshare-user --unshare-pid --unshare-net --die-with-parent \
    --proc /proc --dev /dev \
    --ro-bind /usr /usr --ro-bind /etc /etc \
    --symlink usr/bin /bin --symlink usr/lib /lib --symlink usr/lib64 /lib64 \
    --size 268435456 --tmpfs /sandbox \
    --ro-bind "$root/target" /target \
    --chdir /sandbox \
    --setenv HOME /sandbox \
    --setenv TMPDIR /sandbox \
    --setenv XDG_CONFIG_HOME /sandbox/.config \
    --setenv XDG_DATA_HOME /sandbox/.local/share \
    --setenv PATH /usr/bin:/bin

set -- "$@" --ro-bind "$socket" /sandbox/wayland-0 \
    --setenv XDG_RUNTIME_DIR /sandbox \
    --setenv WAYLAND_DISPLAY wayland-0

out=$(bwrap "$@" -- /target/debug/comarchy --smoke 2>&1) || {
    printf '%s\n' "$out" >&2
    echo "GUI smoke failed inside the sandbox" >&2
    exit 1
}
printf '%s\n' "$out"
case "$out" in
    *SMOKE_OK*) ;;
    *)
        echo "GUI smoke did not finish" >&2
        exit 1
        ;;
esac
echo "GUI smoke finished without using $real_home"
