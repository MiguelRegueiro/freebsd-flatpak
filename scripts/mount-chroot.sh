#!/bin/sh
set -eu

if [ "$#" -lt 3 ]; then
    echo "usage: $0 <chroot-relative-path> <app-files-path> <runtime-files-path> [xdg-runtime-dir] [uid]" >&2
    exit 64
fi

ROOT="$1"
APP_FILES="$2"
RUNTIME_FILES="$3"
HOST_XDG_RUNTIME_DIR="${4:-${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR is not set}}"
UID_VALUE="${5:-$(id -u)}"
BASE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$BASE/$ROOT"
APP_FILES="$BASE/$APP_FILES"
RUNTIME_FILES="$BASE/$RUNTIME_FILES"

mount_if_needed() {
    source="$1"
    target="$2"
    shift 2
    if mount | awk '{print $3}' | grep -qx "$target"; then
        return 0
    fi
    doas "$@" "$source" "$target"
}

mount_if_needed "$RUNTIME_FILES" "$ROOT/usr" mount_nullfs -o ro
mount_if_needed "$APP_FILES" "$ROOT/app" mount_nullfs -o ro
mount_if_needed "$HOST_XDG_RUNTIME_DIR" "$ROOT/run/user/$UID_VALUE" mount_nullfs
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
