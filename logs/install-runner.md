# App-ID Install Runner

Date: 2026-08-16

## Milestone

The manual ref/download/extraction step was replaced with:

```sh
cargo run -- install <app-id>
cargo run -- run <app-id>
```

`install` now:

- refreshes and parses the Flathub OSTree summary
- determines the host Flatpak architecture from `uname -m`
- resolves `app/<app-id>/<arch>/<branch>`, preferring `stable`
- reads the remote app commit metadata
- determines the required runtime and command from `[Application]`
- checks out the real app and runtime payloads into `runtime/`
- reuses existing app/runtime checkouts when `metadata` and `files/` exist

No persistent system configuration was changed.

## Reuse Test

Command:

```sh
cargo run -- install org.gnome.TextEditor
```

Result:

- resolved `app/org.gnome.TextEditor/x86_64/stable`
- reused `runtime/app/org.gnome.TextEditor`
- reused `runtime/org.gnome.Platform-50`
- command from metadata: `gnome-text-editor`

## Third App Test

Command:

```sh
cargo run -- install org.gnome.Characters
cargo run -- run org.gnome.Characters
```

Result:

- resolved `app/org.gnome.Characters/x86_64/stable`
- app commit:
  `0e34fd570ea1ee60c0cb6589142a393861dbaee6df90ee068aef415185e2cfbd`
- runtime from metadata: `org.gnome.Platform/x86_64/50`
- reused `runtime/org.gnome.Platform-50`
- command from metadata: `gnome-characters`
- checkout result: 186 directories, 185 files

The user visually confirmed the Characters window appeared and worked by
copying an emoji from it.

## Process Evidence

For `org.gnome.Characters`, process evidence showed:

- root/cwd/jail under `runtime/chroots/org.gnome.Characters`
- Linux interpreter text mapping from
  `runtime/chroots/org.gnome.Characters/usr/bin/gjs-console`
- live file descriptor to `/var/run/xdg/regueiro/wayland-1`
- live file descriptor to `/tmp/dbus-1Xv4JVVacF`

## Entry Difference

Calculator and Text Editor use Linux ELF entry executables. They continue to
launch through:

```text
/lib64/ld-linux-x86-64.so.2 /app/bin/<command>
```

Characters exposes `/app/bin/gnome-characters` as a symlink to a script with a
Linux shebang:

```text
#!/usr/bin/gjs-console
```

The launcher now resolves the entry on the host for validation. Linux ELF
entries use the dynamic loader path; non-ELF script/shebang entries are executed
directly inside the chroot so the runtime interpreter handles them.

## Signal Cleanup Test

A controlled interrupted run was started:

```sh
target/debug/freebsd-flatpak-poc run org.gnome.Characters
```

SIGTERM was sent to the launcher process. The launcher logged:

```text
received signal 15; app process exited, cleaning up sandbox
```

Post-test checks found no project mounts and no Characters/launcher processes.

## Remaining Hardcoded Items

- The remote is fixed to Flathub.
- The branch resolver prefers `stable`; non-stable multi-branch apps require a
  future selector.
- The sandbox backend remains chroot/nullfs.
- The Linux dynamic loader path remains `/lib64/ld-linux-x86-64.so.2`.
- The runtime path convention remains `runtime/<runtime-name>-<branch>`.
- The environment profile is still GTK-oriented and manually curated.
- `/tmp` is still exposed for this session's D-Bus socket path.
- Fontconfig `/run/host/font-dirs.xml`, AT-SPI, portals, audio, GPU-heavy apps,
  and richer Flatpak permissions are still out of scope.
