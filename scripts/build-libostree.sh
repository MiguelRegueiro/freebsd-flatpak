#!/bin/sh
set -eu

BASE=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
. "$BASE/scripts/install-ui.sh"
VERSION=2026.3
ARCHIVE_SHA256=e560e47631d1f703e9ed3425e8909ccd87fa2992422c07348ca88ec98943c8fb
URL="https://github.com/ostreedev/ostree/releases/download/v$VERSION/libostree-$VERSION.tar.xz"
VENDOR_ROOT="$BASE/target/vendor-ostree"
ARCHIVE="$VENDOR_ROOT/dist/libostree-$VERSION.tar.xz"
SOURCE="$VENDOR_ROOT/src/libostree-$VERSION"
PATCH="$BASE/vendor/libostree/patches/freebsd.patch"
BUILD="$VENDOR_ROOT/build"
BUILD_COMPLETE="$BUILD/.freebsd-flatpak-complete"
PREFIX="$VENDOR_ROOT/prefix"

discard_interrupted_build() {
    signal_status=$1
    trap - HUP INT TERM
    rm -rf "$BUILD"
    exit "$signal_status"
}

ui_heading "Building private libostree $VERSION"

for tool in fetch gmake patch sha256 tar; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "required command not found: $tool" >&2
        exit 1
    }
done

mkdir -p "$VENDOR_ROOT/dist" "$VENDOR_ROOT/src" "$PREFIX"
if [ ! -f "$ARCHIVE" ] || [ "$(sha256 -q "$ARCHIVE")" != "$ARCHIVE_SHA256" ]; then
    PARTIAL="$ARCHIVE.part"
    rm -f "$PARTIAL"
    ui_progress "Downloading source"
    fetch -o "$PARTIAL" "$URL"
    ACTUAL_SHA256=$(sha256 -q "$PARTIAL")
    [ "$ACTUAL_SHA256" = "$ARCHIVE_SHA256" ] || {
        rm -f "$PARTIAL"
        echo "libostree checksum mismatch: expected $ARCHIVE_SHA256, got $ACTUAL_SHA256" >&2
        exit 1
    }
    mv "$PARTIAL" "$ARCHIVE"
fi

PATCH_SHA256=$(sha256 -q "$PATCH")
PREPARED="$SOURCE/.freebsd-flatpak-$PATCH_SHA256"
if [ ! -f "$PREPARED" ]; then
    ui_progress "Preparing source"
    rm -rf "$SOURCE" "$BUILD"
    tar -xf "$ARCHIVE" -C "$VENDOR_ROOT/src"
    patch -d "$SOURCE" -p1 < "$PATCH"
    : > "$PREPARED"
fi

if [ -d "$BUILD" ] && [ ! -f "$BUILD_COMPLETE" ]; then
    rm -rf "$BUILD"
fi
mkdir -p "$BUILD"
rm -f "$BUILD_COMPLETE"
trap 'discard_interrupted_build 129' HUP
trap 'discard_interrupted_build 130' INT
trap 'discard_interrupted_build 143' TERM
cd "$BUILD"

ui_progress "Configuring"
"$SOURCE/configure" \
    --disable-maintainer-mode \
    --prefix="$PREFIX" \
    --with-curl \
    --without-soup \
    --without-soup3 \
    --with-gpgme \
    --without-composefs \
    --without-selinux \
    --without-avahi \
    --without-libmount \
    --without-libsystemd \
    --without-libarchive \
    --disable-rofiles-fuse \
    --disable-introspection \
    --disable-man \
    --disable-gtk-doc \
    --disable-otmpfile

ui_progress "Building"
gmake libglnx-config.h src/libostree/ostree-enumtypes.c
gmake -j "$(sysctl -n hw.ncpu 2>/dev/null || echo 2)" libostree-1.la
ui_progress "Staging"
gmake \
    install-libLTLIBRARIES \
    install-pkgconfigDATA
: > "$BUILD_COMPLETE"
trap - HUP INT TERM
