#!/bin/sh
set -eu

BASE=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$BASE/scripts/install-ui.sh"

if [ "$#" -ne 1 ]; then
    printf 'usage: %s OUTPUT_DIR\n' "$0" >&2
    exit 64
fi

OUTPUT_DIR=$1
mkdir -p "$OUTPUT_DIR"
PORTAL_RESPONSE=$OUTPUT_DIR/portal-bridge.pkg-config.rsp
STATUS_NOTIFIER_RESPONSE=$OUTPUT_DIR/status-notifier-bridge.pkg-config.rsp
trap 'rm -f "$PORTAL_RESPONSE" "$STATUS_NOTIFIER_RESPONSE"' EXIT HUP INT TERM

ui_progress "Portal bridge"
pkg-config --cflags --libs gio-2.0 gio-unix-2.0 glib-2.0 libpipewire-0.3 \
    >"$PORTAL_RESPONSE"
cc -O2 -Wall -Wextra \
    compatibility_helpers/portal_bridge/basic_desktop_portals.c \
    compatibility_helpers/portal_bridge/document_grant_store.c \
    compatibility_helpers/portal_bridge/document_grant_persistence.c \
    compatibility_helpers/portal_bridge/document_id.c \
    compatibility_helpers/portal_bridge/document_mount_backend.c \
    compatibility_helpers/portal_bridge/document_mounts.c \
    compatibility_helpers/portal_bridge/document_portal.c \
    compatibility_helpers/portal_bridge/file_chooser_portal.c \
    compatibility_helpers/portal_bridge/main.c \
    compatibility_helpers/portal_bridge/pipewire_screencast_linker.c \
    compatibility_helpers/portal_bridge/portal_bridge_process.c \
    compatibility_helpers/portal_bridge/portal_request.c \
    compatibility_helpers/portal_bridge/sandbox_document_registration.c \
    compatibility_helpers/portal_bridge/screencast_portal.c \
    compatibility_helpers/portal_bridge/spawn_portal.c \
    -o "$OUTPUT_DIR/portal-bridge" \
    @"$PORTAL_RESPONSE"

ui_progress "Status notifier bridge"
pkg-config --cflags --libs gio-2.0 gio-unix-2.0 glib-2.0 gdk-pixbuf-2.0 \
    >"$STATUS_NOTIFIER_RESPONSE"
cc -O2 -Wall -Wextra \
    compatibility_helpers/status_notifier_bridge/dbusmenu_proxy.c \
    compatibility_helpers/status_notifier_bridge/icon_resolver.c \
    compatibility_helpers/status_notifier_bridge/main.c \
    compatibility_helpers/status_notifier_bridge/status_notifier_item_proxy.c \
    compatibility_helpers/status_notifier_bridge/status_notifier_watcher.c \
    -o "$OUTPUT_DIR/status-notifier-bridge" \
    @"$STATUS_NOTIFIER_RESPONSE"
