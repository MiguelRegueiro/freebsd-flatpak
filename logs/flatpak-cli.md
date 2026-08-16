# Flatpak CLI Milestone

Date: 2026-08-16

## Milestone

The POC now builds a project-local binary named `flatpak`:

```sh
cargo build --bin flatpak
install -m 755 target/debug/flatpak bin/flatpak
```

The supported user-facing commands are:

```sh
bin/flatpak search <query>
bin/flatpak install <app-id>
bin/flatpak list
bin/flatpak run <app-id>
bin/flatpak uninstall <app-id>
bin/flatpak update [app-id...]
```

## State Layout

Generated state remains self-contained and ignored by git:

- `state/apps/<app-id>.ini`
- `state/runtimes/<runtime-ref>.ini`
- `state/runs/<app-id>.ini`

App state records include app ref, app commit, checkout path, architecture,
branch, runtime ref, runtime commit, runtime path, and command.

Runtime state is deduplicated by runtime ref. Uninstall removes an app checkout
and per-app chroot data, but keeps a runtime when another installed app or
extracted app metadata still references it.

## Lifecycle Test

Commands tested:

```sh
bin/flatpak search characters
bin/flatpak install org.gnome.Characters
bin/flatpak install org.gnome.TextEditor
bin/flatpak list
bin/flatpak run org.gnome.Characters
bin/flatpak run org.gnome.TextEditor
bin/flatpak update
bin/flatpak uninstall org.gnome.Characters
bin/flatpak run org.gnome.TextEditor
bin/flatpak install org.gnome.Characters
bin/flatpak update
```

Results:

- `search characters` returned `org.gnome.Characters`.
- `install` wrote clean app/runtime state records.
- both apps shared one runtime record:
  `org.gnome.Platform/x86_64/50`.
- `run` launched both apps through the existing `ChrootNullfsBackend`.
- `update` detected both apps were already current.
- uninstalling Characters removed its app checkout and chroot data while keeping
  the shared GNOME Platform runtime for Text Editor.
- Characters was reinstalled so final CLI state contains both test apps.

## Recovery Test

A controlled SIGKILL test was performed:

1. started `bin/flatpak run org.gnome.TextEditor`
2. confirmed `state/runs/org.gnome.TextEditor.ini` existed with launcher and
   child pids
3. sent SIGKILL to the launcher pid
4. confirmed the run record and chroot mounts were stale
5. ran `bin/flatpak list`

Startup recovery removed the stale run record and all project chroot mounts.
Post-test checks found no project mounts and no app processes.

## Remaining Hardcoded Items

- remote is fixed to Flathub
- branch selection prefers `stable`
- sandbox backend remains chroot/nullfs
- ELF entry launch still uses `/lib64/ld-linux-x86-64.so.2`
- environment profile is still GTK-oriented
- `/tmp` remains exposed for the current D-Bus path
- fontconfig host font mapping, AT-SPI, portals, audio, GPU-heavy apps, and
  broader Flatpak permissions remain out of scope
