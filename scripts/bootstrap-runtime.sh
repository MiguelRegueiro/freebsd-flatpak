#!/bin/sh
set -eu

BASE="$(cd "$(dirname "$0")/.." && pwd)"
cd "$BASE"

if [ "$#" -lt 1 ]; then
    echo "usage: $0 <app-ref> [runtime-ref]" >&2
    echo "example: $0 app/org.gnome.TextEditor/x86_64/stable runtime/org.gnome.Platform/x86_64/50" >&2
    exit 64
fi

ref_dest() {
    ref="$1"
    kind="$(printf '%s' "$ref" | cut -d/ -f1)"
    name="$(printf '%s' "$ref" | cut -d/ -f2)"
    branch="$(printf '%s' "$ref" | cut -d/ -f4)"

    case "$kind" in
        app) printf 'runtime/app/%s\n' "$name" ;;
        runtime) printf 'runtime/%s-%s\n' "$name" "$branch" ;;
        *)
            echo "unsupported ref kind in $ref" >&2
            exit 64
            ;;
    esac
}

cargo build

APP_REF="$1"
target/debug/freebsd-flatpak-poc checkout "$APP_REF" "$(ref_dest "$APP_REF")"

if [ "$#" -ge 2 ]; then
    RUNTIME_REF="$2"
    target/debug/freebsd-flatpak-poc checkout "$RUNTIME_REF" "$(ref_dest "$RUNTIME_REF")"
fi
