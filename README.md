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

The POC now has a project-local `flatpak` CLI:

```sh
bin/flatpak search <query>
bin/flatpak install <app-id>
bin/flatpak list
bin/flatpak run <app-id>
bin/flatpak update
bin/flatpak uninstall <app-id>
```

`install` resolves the app from the Flathub summary for the host architecture,
selects the stable branch when present, reads the remote app metadata to find
the required runtime and command, and checks out both app and runtime into this
project. Existing checkouts and shared runtimes are reused.

Installed-app state is stored under `state/`, while downloaded OSTree objects,
extracted app/runtime payloads, chroots, and caches remain self-contained under
this repository.

`run` reads installed app state, resolves the extracted Flatpak metadata, builds
a per-app chroot under `runtime/chroots/<app-id>`, then uses read-only nullfs
mounts for `/app` and `/usr`, exposes the host Wayland runtime directory, and
starts the Linux app. Linux ELF entries use:

```text
/lib64/ld-linux-x86-64.so.2 /app/bin/<command>
```

Script/shebang entries are executed directly inside the chroot so their Linux
runtime interpreter handles them.

Validated GUI payloads:

- `org.gnome.Calculator`
- `org.gnome.TextEditor`
- `org.gnome.Characters`

The user visually confirmed all three apps created working windows on the host
FreeBSD Hyprland desktop. `org.gnome.Characters` and `org.gnome.TextEditor`
were also tested through the `bin/flatpak` CLI.

## Commands

Build/install the local CLI:

```sh
cargo build --bin flatpak
install -m 755 target/debug/flatpak bin/flatpak
```

Search and install:

```sh
bin/flatpak search characters
bin/flatpak install org.gnome.Characters
bin/flatpak list
```

Run, update, and uninstall:

```sh
bin/flatpak run org.gnome.Characters
bin/flatpak update
bin/flatpak uninstall org.gnome.Characters
```

## Still Hardcoded

- The remote is fixed to Flathub.
- Branch resolution prefers `stable`.
- The sandbox backend is `chroot` plus nullfs/devfs/linprocfs/linsysfs.
- The Linux dynamic loader path is `/lib64/ld-linux-x86-64.so.2`.
- Runtime checkout paths default to `runtime/<runtime-name>-<branch>`.
- App commands are limited to a single executable name or absolute path.
- The environment is a small GTK-oriented V1 profile, including
  `GDK_BACKEND=wayland`, `GTK_USE_PORTAL=0`, and `GSK_RENDERER=cairo`.
- `/tmp` is exposed to preserve the current host session D-Bus socket path.
- `/run/host/font-dirs.xml`, AT-SPI, portals, audio, GPU-heavy apps, and
  richer Flatpak permissions are not handled yet.
- Signal cleanup handles SIGINT, SIGTERM, and SIGHUP. Startup recovery handles
  stale run records/mounts left by SIGKILL or crashes when possible.
