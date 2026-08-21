#!/bin/sh
set -eu

fail() {
    printf 'Error: %s\n' "$*" >&2
    exit 1
}

if [ "$(id -u)" -ne 0 ]; then
    fail "this installer must run as root; rerun it with 'doas ./scripts/install.sh' or 'sudo ./scripts/install.sh'"
fi

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

for command_name in cargo cc env id install mkdir pkg-config su; do
    require_command "$command_name"
done

BUILD_USER=${DOAS_USER:-${SUDO_USER:-}}
[ -n "$BUILD_USER" ] ||
    fail "could not determine the invoking user; run this installer from a normal user account with doas or sudo"
BUILD_UID=$(id -u "$BUILD_USER" 2>/dev/null) ||
    fail "invoking user does not exist: $BUILD_USER"
[ "$BUILD_UID" -ne 0 ] ||
    fail "the build must be owned by a normal user; run this installer with doas or sudo from that account"

BASE=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd) ||
    fail "could not locate the source directory"
INSTALL_BIN=/usr/local/bin
INSTALL_LIBEXEC=/usr/local/libexec/freebsd-flatpak
HELPER_BUILD_DIR=target/release/freebsd-flatpak-helpers
LINUX_CC=${LINUX_CC:-/compat/linux/usr/bin/gcc}
LINUX_BUILD_PATH=/compat/linux/usr/bin:/compat/linux/bin:/usr/bin:/bin

if [ -t 1 ]; then
    HEADING_COLOR=$(printf '\033[1;34m')
    COLOR_RESET=$(printf '\033[0m')
else
    HEADING_COLOR=
    COLOR_RESET=
fi

heading() {
    printf '%s==> %s%s\n' "$HEADING_COLOR" "$1" "$COLOR_RESET"
}

[ -x "$LINUX_CC" ] || fail "Linux compiler not found or not executable: $LINUX_CC"
pkg-config --exists gio-2.0 gio-unix-2.0 glib-2.0 libpipewire-0.3 ||
    fail "missing development packages for GLib/GIO or PipeWire"
PORTAL_FLAGS=$(pkg-config --cflags --libs gio-2.0 gio-unix-2.0 glib-2.0 libpipewire-0.3) ||
    fail "could not determine portal helper compiler flags"

cd "$BASE"
heading "Building FreeBSD Flatpak"
su "$BUILD_USER" -c 'cargo build --locked --release --bin flatpak' ||
    fail "Rust CLI build failed"

su "$BUILD_USER" -c "mkdir -p '$HELPER_BUILD_DIR'" ||
    fail "could not create the helper build directory"
su "$BUILD_USER" -c \
    "cc scripts/portal-bridge.c -o '$HELPER_BUILD_DIR/portal-bridge' $PORTAL_FLAGS" ||
    fail "portal helper build failed"

su "$BUILD_USER" -c \
    "env PATH='$LINUX_BUILD_PATH' '$LINUX_CC' -shared -fPIC -O2 -Wall -Wextra scripts/wayland-drm-devt-shim.c -o '$HELPER_BUILD_DIR/libwayland-drm-devt-shim.so' -ldl" ||
    fail "Wayland DRM helper build failed"
su "$BUILD_USER" -c \
    "env PATH='$LINUX_BUILD_PATH' '$LINUX_CC' -shared -fPIC -O2 -Wall -Wextra scripts/drm-syncobj-errno-shim.c -o '$HELPER_BUILD_DIR/libdrm-syncobj-errno-shim.so' -ldl -pthread" ||
    fail "DRM syncobj helper build failed"
su "$BUILD_USER" -c \
    "env PATH='$LINUX_BUILD_PATH' '$LINUX_CC' -shared -fPIC -O2 -Wall -Wextra scripts/chromium-zygote-drm-preload.c -o '$HELPER_BUILD_DIR/libchromium-zygote-drm-preload.so' -ldl -pthread" ||
    fail "Chromium zygote helper build failed"

for artifact in \
    target/release/flatpak \
    "$HELPER_BUILD_DIR/portal-bridge" \
    "$HELPER_BUILD_DIR/libwayland-drm-devt-shim.so" \
    "$HELPER_BUILD_DIR/libdrm-syncobj-errno-shim.so" \
    "$HELPER_BUILD_DIR/libchromium-zygote-drm-preload.so"
do
    [ -s "$artifact" ] || fail "expected build artifact is missing or empty: $artifact"
done

printf '\n'
heading "Installing"
install -d -o root -g wheel -m 755 "$INSTALL_BIN" "$INSTALL_LIBEXEC" ||
    fail "could not create installation directories"
install -o root -g wheel -m 755 target/release/flatpak "$INSTALL_BIN/flatpak" ||
    fail "could not install CLI"
install -o root -g wheel -m 755 \
    "$HELPER_BUILD_DIR/portal-bridge" \
    "$HELPER_BUILD_DIR/libwayland-drm-devt-shim.so" \
    "$HELPER_BUILD_DIR/libdrm-syncobj-errno-shim.so" \
    "$HELPER_BUILD_DIR/libchromium-zygote-drm-preload.so" \
    "$INSTALL_LIBEXEC/" || fail "could not install helper binaries"

printf '    %s\n' "$INSTALL_BIN/flatpak"
for helper_name in \
    portal-bridge \
    libwayland-drm-devt-shim.so \
    libdrm-syncobj-errno-shim.so \
    libchromium-zygote-drm-preload.so
do
    printf '    %s/%s\n' "$INSTALL_LIBEXEC" "$helper_name"
done

printf '\n'
heading "Installation complete"
