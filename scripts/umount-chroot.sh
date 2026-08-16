#!/bin/sh
set -eu

ROOT="${1:-runtime/chroots/calculator}"
BASE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$BASE/$ROOT"

for mountpoint in "$ROOT/sys" "$ROOT/proc" "$ROOT/dev" "$ROOT/tmp" "$ROOT/run/user/1001" "$ROOT/app" "$ROOT/usr"; do
    if mount | awk '{print $3}' | grep -qx "$mountpoint"; then
        doas umount "$mountpoint"
    fi
done

