#!/bin/sh
set -eu

ROOT="${1:-runtime/chroots/calculator}"
BASE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$BASE/$ROOT"

mount_if_needed() {
    source="$1"
    target="$2"
    shift 2
    if mount | awk '{print $3}' | grep -qx "$target"; then
        return 0
    fi
    doas "$@" "$source" "$target"
}

mount_if_needed "$BASE/runtime/org.gnome.Platform-50/files" "$ROOT/usr" mount_nullfs -o ro
mount_if_needed "$BASE/runtime/app/org.gnome.Calculator/files" "$ROOT/app" mount_nullfs -o ro
mount_if_needed /var/run/xdg/regueiro "$ROOT/run/user/1001" mount_nullfs
mount_if_needed /tmp "$ROOT/tmp" mount_nullfs

if ! mount | awk '{print $3}' | grep -qx "$ROOT/dev"; then
    doas mount -t devfs devfs "$ROOT/dev"
fi
if ! mount | awk '{print $3}' | grep -qx "$ROOT/proc"; then
    doas mount -t linprocfs linprocfs "$ROOT/proc"
fi
if ! mount | awk '{print $3}' | grep -qx "$ROOT/sys"; then
    doas mount -t linsysfs linsysfs "$ROOT/sys"
fi

