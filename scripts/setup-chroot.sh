#!/bin/sh
set -eu

ROOT="${1:-runtime/chroots/calculator}"
BASE="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="$BASE/$ROOT"

mkdir -p "$ROOT"/app "$ROOT"/usr "$ROOT"/dev "$ROOT"/proc "$ROOT"/sys
mkdir -p "$ROOT"/run/user/1001 "$ROOT"/tmp "$ROOT"/var/data "$ROOT"/var/cache "$ROOT"/var/config

make_link() {
    target="$1"
    link="$2"
    if [ ! -e "$link" ] && [ ! -L "$link" ]; then
        ln -s "$target" "$link"
    fi
}

make_link usr/bin "$ROOT/bin"
make_link usr/lib "$ROOT/lib"
make_link usr/lib64 "$ROOT/lib64"
make_link usr/etc "$ROOT/etc"

echo "$ROOT"

