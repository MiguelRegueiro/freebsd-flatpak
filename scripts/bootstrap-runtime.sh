#!/bin/sh
set -eu

BASE="$(cd "$(dirname "$0")/.." && pwd)"
cd "$BASE"

cargo build
target/debug/freebsd-flatpak-poc checkout \
    runtime/org.gnome.Platform/x86_64/50 \
    runtime/org.gnome.Platform-50
target/debug/freebsd-flatpak-poc checkout \
    app/org.gnome.Calculator/x86_64/stable \
    runtime/app/org.gnome.Calculator

