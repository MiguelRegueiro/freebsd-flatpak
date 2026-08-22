#!/bin/sh
set -eu

BASE=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VERSION=2026.3
ARCHIVE_SHA256=e560e47631d1f703e9ed3425e8909ccd87fa2992422c07348ca88ec98943c8fb
URL="https://github.com/ostreedev/ostree/releases/download/v$VERSION/libostree-$VERSION.tar.xz"
VENDOR_ROOT="$BASE/target/vendor-ostree"
ARCHIVE="$VENDOR_ROOT/dist/libostree-$VERSION.tar.xz"
SOURCE="$VENDOR_ROOT/src/libostree-$VERSION"
PATCH="$BASE/vendor/libostree/patches/freebsd.patch"
BUILD="$VENDOR_ROOT/build"
PREFIX="$VENDOR_ROOT/prefix"

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
    echo "==> Downloading libostree $VERSION"
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
    rm -rf "$SOURCE" "$BUILD"
    tar -xf "$ARCHIVE" -C "$VENDOR_ROOT/src"
    patch -d "$SOURCE" -p1 < "$PATCH"
    : > "$PREPARED"
fi

mkdir -p "$BUILD"
cd "$BUILD"

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

gmake libglnx-config.h src/libostree/ostree-enumtypes.c
gmake -j "$(sysctl -n hw.ncpu 2>/dev/null || echo 2)" libostree-1.la
gmake \
    install-libLTLIBRARIES \
    install-pkgconfigDATA
