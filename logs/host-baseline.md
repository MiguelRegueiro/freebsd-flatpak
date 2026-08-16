# Host Baseline

Captured: 2026-08-16

## System

- FreeBSD 15.1-RELEASE amd64
- Kernel/userland: 15.1-RELEASE / 15.1-RELEASE
- User: `regueiro`, uid 1001
- Groups include: `wheel`, `operator`, `video`, `webcamd`

## Linuxulator

- Kernel modules loaded: `linux.ko`, `linux64.ko`, `linux_common.ko`,
  `linprocfs.ko`, `linsysfs.ko`, `fdescfs.ko`, `mqueuefs.ko`
- `compat.linux.osname=Linux`
- `compat.linux.osrelease=5.15.0`
- Mounted Linux compatibility filesystems:
  - `linprocfs` on `/compat/linux/proc`
  - `linsysfs` on `/compat/linux/sys`
  - `devfs` on `/compat/linux/dev`
  - `fdescfs` on `/compat/linux/dev/fd`
  - `tmpfs` on `/compat/linux/dev/shm`
- `/compat/linux` exists and is Rocky Linux based.
- `/compat/linux/usr/bin/uname -a` reports Linux 5.15.0 through Linuxulator.
- Linux scripts with Linux shebangs do not run directly from the FreeBSD shell;
  invoke them through Linux bash if needed.

## Installed Linux GUI Context

- Installed packages include `linux_base-rl9-9.8`, `linux-rl9-gtk3`,
  `linux-rl9-wayland`, `linux-rl9-dri`, `linux-rl9-libdrm`,
  `linux-rl9-libglvnd`, and `linux-discord`.
- Existing Linux Discord process is running through Linuxulator and connected
  to the desktop. This POC must not modify that setup.

## Flatpak / Boxrun / OSTree

- `flatpak` is not installed.
- `boxrun` is not installed.
- Local package lookup did not show `boxrun` or `ostree`.
- `/usr/ports/sysutils/boxrun` is absent.
- `curl`, `fetch`, `bsdtar`, `jq`, `gnupg`, `sqlite3`, `zstd`, Rust, and Cargo
  are available.

## Desktop

- Session: Hyprland on Wayland.
- `XDG_RUNTIME_DIR=/var/run/xdg/regueiro`
- `WAYLAND_DISPLAY=wayland-1`
- `DISPLAY=:0`
- Session DBus: `unix:path=/tmp/dbus-1Xv4JVVacF,...`
- Wayland socket: `/var/run/xdg/regueiro/wayland-1`
- Hyprland control sockets are under
  `/var/run/xdg/regueiro/hypr/5c9377c15f85c50648f35ca5a213754f95b93ca0_1786883686_1629666514/`
- Pulse and PipeWire sockets exist under `/var/run/xdg/regueiro`.

## DRM

- `/dev/dri/card0 -> ../drm/0`
- `/dev/dri/renderD128 -> ../drm/128`

## Persistent System Changes

None so far.

