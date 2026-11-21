#!/bin/sh
# Compares the usage counter with du on real folders. Read only. Folders that nobody writes to should match exactly.
#   scripts/usage-vs-du.sh [PATH...]
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$root"
cargo build --release --example usage 2>&1 | tail -1
bin=target/release/examples/usage

if [ "$#" -eq 0 ]; then
    set -- /usr/share /usr/lib /var/lib "$HOME/Documents" "$root/src" "$root/target/release/deps"
fi

fail=0
for dir in "$@"; do
    [ -d "$dir" ] || continue
    for pass in 1 2; do
        mine=$("$bin" --flat "$dir")
        theirs=$(du -sx -B1 "$dir" 2>/dev/null | cut -f1)
        label=allocated
        if [ "$pass" = 2 ]; then
            mine=$("$bin" --apparent --flat "$dir")
            theirs=$(du -sx -B1 --apparent-size "$dir" 2>/dev/null | cut -f1)
            label=apparent
        fi
        diff=$(( mine > theirs ? mine - theirs : theirs - mine ))
        # 0.5 % of du, in whole bytes
        allowed=$(( theirs / 200 ))
        if [ "$diff" -le "$allowed" ]; then
            verdict=ok
        else
            verdict=FAIL
            fail=1
        fi
        printf '%-8s %-10s mine=%-14s du=%-14s diff=%-10s %s\n' "$verdict" "$label" "$mine" "$theirs" "$diff" "$dir"
    done
done
exit "$fail"
