#!/bin/sh
set -eu

if [ "$#" -lt 1 ]; then
    echo "usage: $0 <app-id> [runner-options...]" >&2
    exit 64
fi

BASE="$(cd "$(dirname "$0")/.." && pwd)"
cd "$BASE"

exec cargo run -- run "$@"
