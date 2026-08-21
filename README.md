# FreeBSD Flatpak

Compatibility layer for running Flatpak applications on FreeBSD using Linuxulator and native FreeBSD integration.

FreeBSD Flatpak downloads applications and runtimes from Flathub, runs their unmodified Linux payloads through Linuxulator, and connects them to the native FreeBSD desktop.

## Features

- Manage Flathub applications with search, install, update, run, and uninstall commands.
- Integrate with Wayland desktops, including application launchers and tray icons.
- Use Mesa GPU acceleration and host audio services.
- Apply filesystem permissions and portal access from Flatpak metadata.
- Share screens through the host portal with restricted PipeWire sessions.

## Requirements

- FreeBSD with Linuxulator enabled and a Linux base under `/compat/linux`.
- A Wayland session with D-Bus and a working `xdg-desktop-portal` backend.
- Rust and C toolchains, GLib/GIO, PipeWire, `fetch`, and `curl`.
- `doas` access for chroot and mount operations.

## Build

```sh
git clone https://github.com/MiguelRegueiro/freebsd-flatpak.git
cd freebsd-flatpak
cargo build --release --bin flatpak
install -m 755 target/release/flatpak bin/flatpak
```

To make installed application launchers available to the desktop session:

```sh
export XDG_DATA_DIRS="$PWD/exports/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
```

## Usage

```sh
bin/flatpak search <query>
bin/flatpak install <app-id>
bin/flatpak list
bin/flatpak permissions <app-id>
bin/flatpak run <app-id> -- <app-arguments>
bin/flatpak update [app-id...]
bin/flatpak uninstall <app-id>
```

## Current limitations

- The project provides a focused compatibility layer rather than the complete upstream Flatpak sandbox model.
- Flathub is currently the only supported remote, and application compatibility varies.
- Wayland is required; X11-only applications are not supported.
- GPU, audio, portal, and ScreenCast availability depends on compatible host services and hardware.

## License

Licensed under the [BSD 2-Clause License](LICENSE).
