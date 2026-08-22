# Private libostree build

The installer downloads upstream libostree 2026.3 from:

`https://github.com/ostreedev/ostree/releases/download/v2026.3/libostree-2026.3.tar.xz`

Pinned SHA-256:

`e560e47631d1f703e9ed3425e8909ccd87fa2992422c07348ca88ec98943c8fb`

`patches/freebsd.patch` contains the FreeBSD portability changes and trims the
generated library source list to the repository, pull, verification, checkout,
fsck, and pruning APIs used by this project. The archive, patched source, build
tree, and private prefix all live below `target/vendor-ostree`; none are tracked.

The library is LGPL-2.0-or-later. Its upstream `COPYING` file is installed next
to the private shared library as `COPYING.libostree`.
