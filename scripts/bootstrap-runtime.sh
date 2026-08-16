#!/bin/sh
set -eu

BASE="$(cd "$(dirname "$0")/.." && pwd)"
cd "$BASE"

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <app-id>" >&2
    echo "example: $0 org.gnome.TextEditor" >&2
    exit 64
fi

cargo build

target/debug/freebsd-flatpak-poc install "$1"
