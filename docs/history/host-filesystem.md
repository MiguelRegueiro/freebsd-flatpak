# Host Filesystem Milestone

Date: 2026-08-17

## Milestone

Flatpak apps now receive controlled access to selected real FreeBSD user
directories through the existing `ChrootNullfsBackend`.

The V1 grant profile is intentionally narrow:

- `~/Downloads` -> `/home/<user>/Downloads`, read-write
- `~/Documents` -> `/home/<user>/Documents`, read-write
- `~/Pictures` -> `/home/<user>/Pictures`, read-only

No whole-home mount is created.

## Argument Mapping

The launcher translates file arguments before entering the chroot:

- `/home/<user>/Documents/file.txt` stays visible at the same sandbox path
- `file:///home/<user>/Documents/file.txt` stays a `file://` URI pointing at
  the sandbox-visible path
- literal desktop field-code placeholders such as `%U` are dropped if a launcher
  passes them through unchanged

Arguments outside configured grants are left unchanged with a warning. The
launcher does not widen filesystem access automatically.

## Runtime Behavior

For every run, the sandbox also writes `user-dirs.dirs` into the app config
directory that is mounted at `/var/config`, so GLib, `xdg-user-dir`, and GTK
file UI can discover the granted user directories. Only granted directories
are published in that file; ungranted host paths remain undisclosed.

The mounts are owned by the existing run instance and are cleaned up by the same
normal-exit, signal, and stale-mount recovery paths as the app/runtime/Wayland
mounts.

Cleanup now retries normal unmounts before failing. If a read-only project-owned
mount remains busy during normal cleanup or stale recovery, the launcher falls
back to `umount -f` for that read-only mount only. Writable host directory
mounts are not force-unmounted.

Signal handlers are installed before mounts are created. SIGINT and SIGTERM are
forwarded to the app process and then cleaned up. SIGHUP is caught but not
forwarded because desktop launch helpers may exit independently of the GUI app.

## Validation

Text Editor was launched through the generic CLI with a real host file:

```sh
bin/flatpak run org.gnome.TextEditor -- /home/regueiro/Documents/freebsd-flatpak-poc-open-edit.txt
```

The app opened a Wayland window and saved changes back to the FreeBSD host file
under `~/Documents`.

A second Text Editor run saved a new file to:

```text
/home/regueiro/Documents/freebsd-flatpak-poc-new-save.txt
```

The new file was visible and readable on the FreeBSD host immediately after
save. After Text Editor exited, no mounts remained under
`runtime/chroots/org.gnome.TextEditor` and no run records remained under
`state/runs`.

The exported desktop entry for Text Editor preserves file arguments as:

```text
Exec=/home/regueiro/freebsd-flatpak-poc/bin/flatpak run org.gnome.TextEditor -- %U
```

Running the substituted command directly with a local `file://` URI opened the
same host file successfully. Attempts to use `gtk-launch` from the Codex command
wrapper are not a valid GUI-lifetime test here: `ktrace` showed the spawned
`flatpak` process receiving `SIGKILL` while waiting for the app child, which
necessarily leaves stale mounts for startup recovery. Recovery cleaned that
state successfully.

## Metadata-Derived Permissions

The fixed V1 profile was replaced with a resolver for each installed app's
Flatpak metadata:

```ini
[Context]
filesystems=...
```

Supported filesystem names:

- `home`
- `host`
- `xdg-desktop`
- `xdg-documents`
- `xdg-download`
- `xdg-music`
- `xdg-pictures`
- `xdg-public-share`
- `xdg-templates`
- `xdg-videos`

The resolver parses `:ro`, `:rw`, and `:create`. Read-only permissions become
read-only nullfs mounts; read-write and create permissions become read-write
nullfs mounts. Missing paths are skipped unless `:create` is present.

`home` maps to the user's home path inside the sandbox at `/home/<user>`.
`host` is implemented as a V1-safe expansion of common host roots and home child
directories. It does not mount `/` over the chroot and skips the project
directory to avoid recursive nullfs mounts.

Unsupported entries such as `xdg-run/gvfsd` are reported by the launcher and the
inspector, but are not treated as host filesystem grants.

Inspection command:

```sh
bin/flatpak permissions <app-id>
```

Validation:

- `org.gnome.Calculator` has no metadata `filesystems=` entry. A run created no
  user-file nullfs mounts, only the runtime/app/session mounts.
- `org.gnome.TextEditor` declares `filesystems=xdg-run/gvfsd;host;xdg-run/gvfs;`.
  A run mounted metadata-derived `host` grants including `Documents`,
  `Downloads`, `Pictures`, `/media`, and `/mnt`. The project directory was not
  mounted into the sandbox.
- After both apps exited, no mounts remained under `runtime/chroots` and no run
  records remained under `state/runs`.
