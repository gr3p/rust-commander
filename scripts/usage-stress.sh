#!/bin/bash
# Stress checks for the usage counter that need mounts, limits or a lot of files. Needs no root: mounts are made in a
# user namespace with `unshare -rm`. Everything is made under a scratch folder and removed again.
#   scripts/usage-stress.sh
set -euo pipefail
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$root"
cargo build --release --example usage 2>&1 | tail -1
bin="$root/target/release/examples/usage"
work=$(mktemp -d /tmp/rc-usage-stress.XXXXXX)
trap 'chmod -R u+rwx "$work" 2>/dev/null; rm -rf "$work"' EXIT
fail=0
check() { # name, ok
    if [ "$2" = 1 ]; then echo "ok    $1"; else echo "FAIL  $1"; fail=1; fi
}

# D1: another mount is shown but not entered, unless asked for.
mkdir -p "$work/d1/mnt" && echo local > "$work/d1/local"
out=$(unshare -rm bash -c "mount -t tmpfs -o size=8m none '$work/d1/mnt' && head -c 4000000 /dev/zero > '$work/d1/mnt/big' && '$bin' --apparent '$work/d1' 2>&1; echo ---; '$bin' --apparent --cross '$work/d1' 2>&1" 2>&1 || true)
apart=$(echo "$out" | sed -n '/^---$/q;p' | awk -F'\t' '$2=="total"{print $1}')
cross=$(echo "$out" | sed -n '/^---$/,$p' | awk -F'\t' '$2=="total"{print $1}')
check "D1 mount not entered (total=$apart)" "$([ "${apart:-x}" -lt 100 ] 2>/dev/null && echo 1 || echo 0)"
check "D1 mount entered with --cross (total=$cross)" "$([ "${cross:-0}" -ge 4000000 ] 2>/dev/null && echo 1 || echo 0)"
echo "$out" | grep -q "1 other mounts" && check "D1 other mount reported" 1 || check "D1 other mount reported" 0

# D2: a bind mount of the folder inside itself is not counted twice and does not loop.
mkdir -p "$work/d2/sub/loop" && head -c 100000 /dev/zero > "$work/d2/file"
plain=$("$bin" --apparent --flat "$work/d2")
looped=$(unshare -rm bash -c "mount --bind '$work/d2' '$work/d2/sub/loop' && timeout 20 '$bin' --apparent --flat '$work/d2'")
check "D2 bind loop terminates and counts once ($plain vs $looped)" "$([ "$plain" = "$looped" ] && echo 1 || echo 0)"

# D3: few file descriptors are enough, one open folder per thread.
for i in $(seq 1 300); do mkdir -p "$work/d3/a$((i % 7))/b$i"; echo x > "$work/d3/a$((i % 7))/b$i/f"; done
res=$(bash -c "ulimit -n 32; '$bin' --apparent --threads 8 '$work/d3' 2>&1" | tail -2 | tr '\n' ' ')
echo "$res" | grep -q "total" && ! echo "$res" | grep -qi "too many\|error" && check "D3 ulimit -n 32 ($res)" 1 || check "D3 ulimit -n 32 ($res)" 0

# D4: memory follows the width of the tree, not the number of files.
mkdir -p "$work/d4"
python3 - "$work/d4" <<'PY'
import os, sys
base = sys.argv[1]
for d in range(20000):
    path = os.path.join(base, "d%d" % (d % 100), "e%d" % d)
    os.makedirs(path, exist_ok=True)
    for f in range(50):
        open(os.path.join(path, "f%d" % f), "w").close()
PY
mem=$( { /usr/bin/time -f "%M %e" "$bin" --apparent --flat "$work/d4" >/dev/null; } 2>&1 | tail -1)
kb=${mem%% *}
check "D4 1M files: max RSS ${kb} kB, ${mem#* } s" "$([ "$kb" -lt 100000 ] && echo 1 || echo 0)"

# D8: a root that does not exist, is a file or is a link
"$bin" "$work/none" >/dev/null 2>&1 && check "D8 missing root fails" 0 || check "D8 missing root fails" 1
ln -s "$work/d1" "$work/link"
"$bin" "$work/link" >/dev/null 2>&1 && check "D8 symlink root refused" 0 || check "D8 symlink root refused" 1

exit "$fail"
