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
bin/flatpak permissions <app-id>
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

`install` and `update` export each app's real Flatpak desktop metadata into the
project-local XDG data directory at `exports/share`. Exported desktop entries
preserve names, icons, categories, MIME associations, metainfo, and desktop
actions where possible, but rewrite `Exec=` so launchers call:

```text
/home/regueiro/freebsd-flatpak-poc/bin/flatpak run <app-id>
```

`uninstall` removes only the exported files recorded for that app. Generated
desktop/icon caches may remain under `exports/share`.

`run` reads installed app state, resolves the extracted Flatpak metadata, builds
a per-app chroot under `runtime/chroots/<app-id>`, then uses read-only nullfs
mounts for `/app` and `/usr`, exposes the host Wayland runtime directory, and
starts the Linux app. Linux ELF entries use:

```text
/lib64/ld-linux-x86-64.so.2 /app/bin/<command>
```

Script/shebang entries are executed directly inside the chroot so their Linux
runtime interpreter handles them.

Host filesystem access is derived from each installed app's Flatpak metadata
`[Context] filesystems=` entry. The V1 resolver supports:

- `home`
- `host`
- `xdg-desktop`
- `xdg-documents`
- `xdg-download`
- `xdg-music`
- `xdg-pictures`
- `xdg-public-share`
- `xdg-videos`

The `:ro`, `:rw`, and `:create` suffixes are parsed. Access is mapped to
per-run nullfs mounts, with read-only nullfs used for `:ro`. File and `file://`
URI arguments under granted directories are translated to the sandbox-visible
path before the Linux app starts. The sandbox also writes
`/var/config/user-dirs.dirs` so GTK file UI has sensible user directory paths.

For broad `home`/`host` access, the resolver expands directories into child
nullfs mounts when a direct mount would recurse into this project directory.
The project directory itself is skipped.

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
bin/flatpak permissions org.gnome.Characters
```

Run, update, and uninstall:

```sh
bin/flatpak run org.gnome.Characters
bin/flatpak update
bin/flatpak uninstall org.gnome.Characters
```

Use the project-local desktop exports for launcher testing:

```sh
export XDG_DATA_DIRS=/home/regueiro/freebsd-flatpak-poc/exports/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}
```

## Still Hardcoded

- The remote is fixed to Flathub.
- Branch resolution prefers `stable`.
- The sandbox backend is `chroot` plus nullfs/devfs/linprocfs/linsysfs.
- The Linux dynamic loader path is `/lib64/ld-linux-x86-64.so.2`.
- Runtime checkout paths default to `runtime/<runtime-name>-<branch>`.
- App commands are limited to a single executable name or absolute path.
- Exported desktop `Exec=` rewriting drops the original executable token and
  preserves only trailing arguments/field codes.
- `DBusActivatable=true` is rewritten to `false` so host launchers use our
  `Exec=` path instead of trying host D-Bus activation.
- Exported D-Bus service files and GNOME Shell search providers are skipped for
  now because they point at `/app/...` host-incompatible commands.
- The environment is a small GTK-oriented V1 profile, including
  `GDK_BACKEND=wayland`, `GTK_USE_PORTAL=0`, and `GSK_RENDERER=cairo`.
- `/tmp` is exposed to preserve the current host session D-Bus socket path.
- Host filesystem grants are derived from app metadata, but only the common
  V1 filesystem names listed above are implemented. `xdg-run/...`,
  `host-os`, `host-etc`, `host-root`, arbitrary absolute paths, and
  homedir-relative arbitrary paths are reported as unsupported for now.
- `host` is not a single `/` mount. It expands to common host roots and child
  home-directory mounts so it cannot overwrite the Linux runtime or recursively
  mount the project into its own chroot.
- File arguments outside the metadata-derived grants are left unchanged with a
  warning; they are not made visible by mounting broader host paths.
- `/run/host/font-dirs.xml`, AT-SPI, portals, audio, GPU-heavy apps, and
  richer Flatpak permissions are not handled yet.
- Signal cleanup handles SIGINT and SIGTERM by forwarding them to the app and
  unmounting afterward. SIGHUP is caught but not forwarded so desktop launcher
  parent exits do not kill GUI apps. Startup recovery handles stale run
  records/mounts left by SIGKILL or crashes when possible.
