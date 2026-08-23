#!/bin/sh
set -eu

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
    echo "usage: $0 <label> <flatpak-binary> [empty-root]" >&2
    exit 64
fi

LABEL=$1
BINARY=$2
ROOT=${3:-$(mktemp -d /tmp/freebsd-flatpak-benchmark.XXXXXX)}

[ -x "$BINARY" ] || {
    echo "not an executable: $BINARY" >&2
    exit 66
}
mkdir -p "$ROOT"

DEFAULT_ROUTE=$(netstat -rn -f inet | awk '$1 == "default" { print $4; exit }')
[ -n "$DEFAULT_ROUTE" ] || DEFAULT_ROUTE=unknown

printf 'Benchmark: %s\n' "$LABEL"
printf 'Root: %s\n' "$ROOT"
printf 'Binary: %s\n' "$BINARY"
printf 'Default route: %s\n' "$DEFAULT_ROUTE"

/usr/bin/time -h -o "$ROOT/time.txt" \
    env \
    FREEBSD_FLATPAK_DATA_DIR="$ROOT/data" \
    FREEBSD_FLATPAK_CACHE_DIR="$ROOT/cache" \
    FREEBSD_FLATPAK_RUNTIME_DIR="$ROOT/run" \
    FREEBSD_FLATPAK_APP_DATA_DIR="$ROOT/app-data" \
    HOME="$ROOT/home" \
    XDG_DATA_HOME="$ROOT/xdg-data" \
    XDG_CONFIG_HOME="$ROOT/xdg-config" \
    XDG_CACHE_HOME="$ROOT/xdg-cache" \
    FREEBSD_FLATPAK_TRACE_RESOLUTION=1 \
    FREEBSD_FLATPAK_BENCHMARK=1 \
    "$BINARY" install org.gnome.TextEditor

cat "$ROOT/time.txt"
du -sh "$ROOT/data" "$ROOT/cache"
printf 'Results retained at %s\n' "$ROOT"
