#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <chroot-relative-path>" >&2
    exit 64
fi

ROOT="$1"
BASE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$BASE/$ROOT"

for mountpoint in "$ROOT/sys" "$ROOT/proc" "$ROOT/dev" "$ROOT/tmp" "$ROOT"/run/user/* "$ROOT/app" "$ROOT/usr"; do
    if mount | awk '{print $3}' | grep -qx "$mountpoint"; then
        doas umount "$mountpoint"
    fi
done
