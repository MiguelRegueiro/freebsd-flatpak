#!/bin/sh
set -eu

ROOT="${1:-runtime/chroots/calculator}"
BASE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$BASE/$ROOT"

exec doas chroot -u 1001 -g 1001 "$ROOT" \
    /usr/bin/env -i \
    HOME=/var/data \
    USER=regueiro \
    LOGNAME=regueiro \
    SHELL=/bin/sh \
    XDG_RUNTIME_DIR=/run/user/1001 \
    WAYLAND_DISPLAY=wayland-1 \
    DISPLAY=:0 \
    DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/dbus-1Xv4JVVacF \
    LANG=C.UTF-8 \
    LC_ALL=C.UTF-8 \
    GDK_BACKEND=wayland \
    GSK_RENDERER=cairo \
    GTK_USE_PORTAL=0 \
    XDG_DATA_HOME=/var/data \
    XDG_CONFIG_HOME=/var/config \
    XDG_CACHE_HOME=/var/cache \
    XDG_DATA_DIRS=/app/share:/usr/share:/usr/share/runtime/share \
    GI_TYPELIB_PATH=/app/lib/girepository-1.0 \
    LD_LIBRARY_PATH=/app/lib:/usr/lib/x86_64-linux-gnu:/usr/lib \
    PATH=/app/bin:/usr/bin:/bin \
    /lib64/ld-linux-x86-64.so.2 /app/bin/gnome-calculator
