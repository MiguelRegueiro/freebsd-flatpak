# FreeBSD Flatpak POC

V1 goal: launch an unmodified Linux Flatpak application through FreeBSD
Linuxulator, using FreeBSD-native sandbox/runtime setup instead of a VM.

This workspace is intentionally self-contained. Downloads, extracted Flatpak
runtime/app data, scripts, logs, transient mount targets, and host-side Rust
code live here unless a system-level dependency is unavoidable.

## Current Constraints

- No bhyve, QEMU, or VM.
- Do not fork or significantly modify Linuxulator.
- Do not implement generic Linux syscall compatibility.
- Prefer replacing Flatpak's Linux-specific sandbox layer with FreeBSD
  mechanisms.
- Use existing Linuxulator for Linux ELF execution.
- Keep persistent system changes minimal, explicit, and reversible.

## Current Status

The POC now has a generic payload launcher for already-extracted Flatpak app
and runtime checkouts:

```sh
cargo run -- run <app-id>
```

The launcher reads `runtime/app/<app-id>/metadata`, resolves the app command
and runtime ref, builds a per-app chroot under `runtime/chroots/<app-id>`, then
uses read-only nullfs mounts for `/app` and `/usr`, exposes the host Wayland
runtime directory, and starts the Linux app through the runtime loader:

```text
/lib64/ld-linux-x86-64.so.2 /app/bin/<command>
```

Validated GUI payloads:

- `org.gnome.Calculator`
- `org.gnome.TextEditor`

The user visually confirmed both apps created working windows on the host
FreeBSD Hyprland desktop.

## Commands

Inspect Flathub refs:

```sh
cargo run -- inspect app/org.gnome.TextEditor/x86_64/stable
```

Extract an app or runtime:

```sh
cargo run -- checkout app/org.gnome.TextEditor/x86_64/stable runtime/app/org.gnome.TextEditor
cargo run -- checkout runtime/org.gnome.Platform/x86_64/50 runtime/org.gnome.Platform-50
```

Run an already-extracted app:

```sh
cargo run -- run org.gnome.TextEditor
```

Optional manual overrides are available for experiments:

```sh
cargo run -- run <app-id> --app-dir <path> --runtime-dir <path> --entry <executable>
```

## Still Hardcoded

- The sandbox backend is `chroot` plus nullfs/devfs/linprocfs/linsysfs.
- The Linux dynamic loader path is `/lib64/ld-linux-x86-64.so.2`.
- Runtime checkout paths default to `runtime/<runtime-name>-<branch>`.
- App commands are limited to a single executable name or absolute path.
- The environment is a small GTK-oriented V1 profile, including
  `GDK_BACKEND=wayland`, `GTK_USE_PORTAL=0`, and `GSK_RENDERER=cairo`.
- `/tmp` is exposed to preserve the current host session D-Bus socket path.
- `/run/host/font-dirs.xml`, AT-SPI, portals, audio, GPU-heavy apps, and
  richer Flatpak permissions are not handled yet.
