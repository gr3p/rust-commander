#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$root"

if ! command -v bwrap >/dev/null 2>&1; then
    echo "bubblewrap is missing; destructive tests were not run" >&2
    exit 1
fi

if pkg-config --exists gtk4; then
    :
else
    prefix="$HOME/.local/opt/gtk4-prefix"
    if [ -d "$prefix/usr/lib/x86_64-linux-gnu/pkgconfig" ]; then
        export PKG_CONFIG_PATH="$prefix/usr/lib/x86_64-linux-gnu/pkgconfig:$prefix/usr/lib/pkgconfig:$prefix/usr/share/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
    fi
fi

cargo test --offline --no-run --test acceptance --message-format=json > /tmp/rust-commander-acceptance.json
bin=$(python3 -c '
import json
path=""
for line in open("/tmp/rust-commander-acceptance.json"):
    try:
        obj=json.loads(line)
    except json.JSONDecodeError:
        continue
    exe=obj.get("executable")
    if exe and obj.get("target",{}).get("name")=="acceptance":
        path=exe
print(path)
')
if [ -z "$bin" ] || [ ! -x "$bin" ]; then
    echo "acceptance binary was not built" >&2
    exit 1
fi

rel=${bin#"$root"/}
real_home=$HOME
exec bwrap \
    --unshare-user --unshare-pid --unshare-net --unshare-uts --unshare-ipc \
    --die-with-parent \
    --proc /proc \
    --dev /dev \
    --ro-bind /usr /usr \
    --ro-bind /etc /etc \
    --symlink usr/bin /bin \
    --symlink usr/lib /lib \
    --symlink usr/lib64 /lib64 \
    --size 268435456 --tmpfs /sandbox \
    --ro-bind "$root/target" /target \
    --chdir /sandbox \
    --setenv HOME /sandbox \
    --setenv TMPDIR /sandbox \
    --setenv XDG_CONFIG_HOME /sandbox/.config \
    --setenv XDG_DATA_HOME /sandbox/.local/share \
    --setenv PATH /usr/bin:/bin \
    --setenv RUST_COMMANDER_REQUIRE_SANDBOX 1 \
    --setenv RUST_COMMANDER_REAL_HOME "$real_home" \
    "/target/${rel#target/}"
