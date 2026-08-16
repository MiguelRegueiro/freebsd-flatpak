# V1 Result

Date: 2026-08-16

## Target

- App: `org.gnome.Calculator/x86_64/stable`
- Runtime: `org.gnome.Platform/x86_64/50`
- Source: Flathub OSTree repository at `https://dl.flathub.org/repo/`
- App commit: `83ef900d1ed3f3ff2edfb6e894d4a2120da21848f1bbf16c57d9b1b299c59f5b`
- Runtime commit: `a8da766dd0273a67539d2b98358ed6809d8e729280baf63428108837135229d3`

## What Was Proven

The POC launched the unmodified Linux Flatpak Calculator app against the
unmodified GNOME Platform Flatpak runtime through FreeBSD Linuxulator, and the
app created a real individual Wayland window on the host Hyprland desktop.

The first launch used absolute paths without a chroot to prove that the
extracted app/runtime could execute with the Linux runtime loader and connect
to the host Wayland socket.

The second launch used a FreeBSD-native chroot/nullfs sandbox:

- `/usr` was a read-only nullfs mount of `runtime/org.gnome.Platform-50/files`
- `/app` was a read-only nullfs mount of
  `runtime/app/org.gnome.Calculator/files`
- `/dev`, `/proc`, and `/sys` were devfs/linprocfs/linsysfs mounts
- `/run/user/1001` exposed the real host Wayland runtime directory
- `/tmp` exposed the existing session DBus socket path
- The process ran as uid/gid `1001`

The user visually confirmed that the chrooted Calculator window appeared and
was interactive.

## Process Evidence

During the chrooted launch:

- Process command: `/lib64/ld-linux-x86-64.so.2 /app/bin/gnome-calculator`
- `procstat` showed process root and cwd inside
  `runtime/chroots/calculator`
- `procstat` showed the text object resolved through the chroot:
  `runtime/chroots/calculator/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2`
- `sockstat` showed a live connection to
  `/var/run/xdg/regueiro/wayland-1`
- `sockstat` showed a live connection to the session DBus socket

## Checkout Tool

`src/runtime.rs` implements the narrow V1 OSTree checkout path:

- fetch refs and commits from Flathub
- parse OSTree commit and dirtree GVariants
- fetch objects into `downloads/objects`
- decode archive-z2 regular file and symlink objects
- materialize checkouts under `runtime/`

Verified checkouts:

- App: 258 directories, 1235 files
- Runtime: 3834 directories, 21223 files

## Cleanup

The launched Calculator processes were stopped after validation.

The temporary chroot mounts were removed with `scripts/umount-chroot.sh`.

No persistent system configuration changes were made.

## Known V1 Gaps

- This is not upstream Flatpak CLI integration yet.
- `boxrun` was not available locally, so V1 used `chroot` plus nullfs/devfs.
- No portal, audio, camera, GPU-heavy, or browser-class app support was proven.
- Runtime networking from inside the chroot lacked DNS setup during the
  Calculator currency-rate fetch. That was non-blocking for the GUI proof.
- Fontconfig warned about missing `/run/host/font-dirs.xml`.
- AT-SPI inside the chroot tried the host absolute path
  `/var/run/xdg/regueiro/at-spi/bus`; this should be mapped or disabled in a
  cleaner sandbox profile.

