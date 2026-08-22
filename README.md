# FreeBSD Flatpak

Compatibility layer for running Flatpak applications on FreeBSD using Linuxulator and native FreeBSD integration.

FreeBSD Flatpak downloads applications and runtimes from Flathub, runs their unmodified Linux payloads through Linuxulator, and connects them to the native FreeBSD desktop.

## Features

- Manage Flathub applications with search, install, update, downgrade, run, and uninstall commands.
- Integrate with Wayland desktops, including application launchers and tray icons.
- Use Mesa GPU acceleration and host audio services.
- Apply filesystem permissions and portal access from Flatpak metadata.
- Share screens through the host portal with restricted PipeWire sessions.

## Requirements

- FreeBSD with Linuxulator enabled and a Linux base under `/compat/linux`.
- A Wayland session with D-Bus and a working `xdg-desktop-portal` backend.
- FreeBSD `pkg`; the installer installs missing build/runtime packages.
- `doas` access for chroot and mount operations.

## Build

```sh
git clone https://github.com/MiguelRegueiro/freebsd-flatpak.git
cd freebsd-flatpak
doas ./scripts/install.sh
```

Use `sudo ./scripts/install.sh` instead if `sudo` is your preferred privilege
elevation tool.

This installs the CLI at `/usr/local/bin/flatpak` and the native/Linux helper
binaries under `/usr/local/libexec/freebsd-flatpak`. It also downloads a
checksum-pinned libostree release, applies the small FreeBSD patchset, builds
it under `target/`, and installs it privately in that directory; no system
libostree package or manual library setup is needed. Application
launchers, icons, and metadata are published into the normal per-user XDG data
paths; no `XDG_DATA_DIRS` change is needed.

## Usage

```sh
flatpak search <query>
flatpak remote-info [--log | --commit=COMMIT] flathub <app-id>
flatpak install <app-id>
flatpak list
flatpak ps [--columns=instance,application,pid,child-pid]
flatpak permissions <app-id>
flatpak repair
flatpak prune
flatpak run <app-id> -- <app-arguments>
flatpak update [app-id...]
flatpak update --commit=COMMIT <app-id>
flatpak uninstall <app-id>
```

`remote-info --log` shows the OSTree commit history available for an app ref,
including apps that are not installed. Use `remote-info --commit=COMMIT` to
inspect a historical commit and `update --commit=COMMIT` to move an installed
app to that exact commit. A normal `update` follows the current Flathub tip.

`flatpak ps` lists active application instances, including their instance ID,
wrapper PID, application, and runtime. Use `--columns` to select debugging
fields such as `child-pid`.

User installations use `$XDG_DATA_HOME/freebsd-flatpak` for the private OSTree
repository and transactional deployments, `$XDG_CACHE_HOME/freebsd-flatpak`
for signed remote metadata, `$XDG_RUNTIME_DIR/freebsd-flatpak` for transient
run state, and `~/.var/app/<app-id>` for persistent application data. Standard
XDG defaults are used when the data or cache variables are unset.

## Current limitations

- The project provides a focused compatibility layer rather than the complete upstream Flatpak sandbox model.
- Flathub is currently the only supported remote, and application compatibility varies.
- Wayland is required; X11-only applications are not supported.
- GPU, audio, portal, and ScreenCast availability depends on compatible host services and hardware.

## License

FreeBSD Flatpak is licensed under the [BSD 2-Clause License](LICENSE).
libostree is LGPL-2.0-or-later; the pinned upstream source and local patch are
documented in `vendor/libostree`.
