# System Changes

## 2026-08-16

No persistent system configuration changes have been made.

Non-system persistent side effect: `cargo build` used the normal Cargo cache
under `/home/regueiro/.cargo` for downloaded Rust crates. Project build output
is under `target/`.

Temporary runtime mounts used for the successful chroot launch experiment:

- `runtime/org.gnome.Platform-50/files` mounted read-only with nullfs at
  `runtime/chroots/calculator/usr`
- `runtime/app/org.gnome.Calculator/files` mounted read-only with nullfs at
  `runtime/chroots/calculator/app`
- `/var/run/xdg/regueiro` mounted with nullfs at
  `runtime/chroots/calculator/run/user/1001`
- `/tmp` mounted with nullfs at `runtime/chroots/calculator/tmp` only to expose
  the existing session DBus socket path
- `devfs` mounted at `runtime/chroots/calculator/dev`
- `linprocfs` mounted at `runtime/chroots/calculator/proc`
- `linsysfs` mounted at `runtime/chroots/calculator/sys`

These mounts were removed with `scripts/umount-chroot.sh` after visual
validation. A post-cleanup mount table check found no remaining mounts under
`runtime/chroots/calculator`.

## 2026-08-16 Generic Launcher Follow-up

No persistent system configuration changes were made.

Additional non-system project data was created under this repository:

- `runtime/app/org.gnome.TextEditor`
- additional Flathub OSTree objects under `downloads/objects`
- per-app chroot directory `runtime/chroots/org.gnome.TextEditor`

Temporary mounts used by the generic launcher for Text Editor:

- `runtime/org.gnome.Platform-50/files` mounted read-only at
  `runtime/chroots/org.gnome.TextEditor/usr`
- `runtime/app/org.gnome.TextEditor/files` mounted read-only at
  `runtime/chroots/org.gnome.TextEditor/app`
- `/var/run/xdg/regueiro` mounted at
  `runtime/chroots/org.gnome.TextEditor/run/user/1001`
- `/tmp` mounted at `runtime/chroots/org.gnome.TextEditor/tmp`
- `devfs`, `linprocfs`, and `linsysfs` mounted in the chroot

These mounts are created by `cargo run -- run <app-id>` and are unmounted by
the Rust launcher after normal app exit.

## 2026-08-16 App-ID Install Follow-up

No persistent system configuration changes were made.

Additional non-system project data was created under this repository:

- refreshed Flathub summary at `downloads/summary`
- additional Flathub OSTree objects under `downloads/objects`
- `runtime/app/org.gnome.Characters`
- per-app chroot directory `runtime/chroots/org.gnome.Characters`

Temporary mounts used by the generic launcher for Characters:

- `runtime/org.gnome.Platform-50/files` mounted read-only at
  `runtime/chroots/org.gnome.Characters/usr`
- `runtime/app/org.gnome.Characters/files` mounted read-only at
  `runtime/chroots/org.gnome.Characters/app`
- `/var/run/xdg/regueiro` mounted at
  `runtime/chroots/org.gnome.Characters/run/user/1001`
- `/tmp` mounted at `runtime/chroots/org.gnome.Characters/tmp`
- `devfs`, `linprocfs`, and `linsysfs` mounted in the chroot

Normal-exit cleanup and a controlled SIGTERM cleanup test both removed all
project mounts.

## 2026-08-16 Flatpak CLI Follow-up

No persistent system configuration changes were made.

The project-local CLI binary was built and installed inside the repository:

- `target/debug/flatpak`
- `bin/flatpak`

Additional ignored project-local state was created:

- `state/apps/org.gnome.Characters.ini`
- `state/apps/org.gnome.TextEditor.ini`
- `state/runtimes/org.gnome.Platform_x86_64_50.ini`

Lifecycle tests temporarily mounted Characters and Text Editor chroots and then
cleaned them up. A controlled SIGKILL recovery test left stale Text Editor
mounts intentionally, then verified `bin/flatpak list` startup recovery removed
them.

Post-test checks found no remaining project mounts, no run records, and no
Flatpak app processes.

## 2026-08-16 Desktop Integration Follow-up

No persistent system configuration changes were made.

Additional ignored project-local data was created under this repository:

- `exports/share/applications/*.desktop`
- `exports/share/icons/hicolor/...`
- `exports/share/metainfo/*.xml`
- `exports/share/applications/mimeinfo.cache`
- `exports/share/icons/hicolor/icon-theme.cache`
- `state/exports/*.list`

A temporary Hyprland runtime environment variable was set so newly launched
session processes can discover the project-local desktop exports:

```sh
hyprctl keyword env XDG_DATA_DIRS,/home/regueiro/freebsd-flatpak-poc/exports/share:/usr/local/share:/usr/share
```

This changes only the current Hyprland session environment. It is reversible by
setting `XDG_DATA_DIRS` back to the desired value or by restarting the session.

## 2026-08-17 Host Filesystem Follow-up

No persistent system configuration changes were made.

The new V1 host filesystem layer uses per-run nullfs mounts under each app's
project-local chroot only. The default grant profile is:

- `/home/regueiro/Downloads` read-write
- `/home/regueiro/Documents` read-write
- `/home/regueiro/Pictures` read-only

These mounts are created only while an app is running and are removed by the
existing sandbox cleanup path.

Validation created two host files under `/home/regueiro/Documents`:

- `/home/regueiro/Documents/freebsd-flatpak-poc-open-edit.txt`
- `/home/regueiro/Documents/freebsd-flatpak-poc-new-save.txt`

Both were edited/saved by Linux Flatpak Text Editor through the per-run
read-write `Documents` nullfs mount. Post-run checks found no remaining project
chroot mounts and no run records.

## 2026-08-17 Filesystem Permission Semantics Follow-up

No persistent system configuration changes were made.

The launcher now derives host filesystem nullfs mounts from each app's extracted
Flatpak metadata instead of using a fixed Downloads/Documents/Pictures profile.

Temporary validation mounts:

- `org.gnome.Calculator` declared no `filesystems=` permissions, so no
  user-file nullfs mounts were created.
- `org.gnome.TextEditor` declared `host`, so the run temporarily mounted common
  host roots and home child directories including `/home/regueiro/Documents`,
  `/home/regueiro/Downloads`, `/home/regueiro/Pictures`, `/media`, and `/mnt`.

The project directory `/home/regueiro/freebsd-flatpak-poc` was deliberately
skipped during broad `host` expansion to avoid a recursive nullfs mount.

Post-run checks found no remaining project chroot mounts and no run records.

## 2026-08-17 Host Audio Follow-up

No persistent system configuration changes were made.

The current host audio setup was inspected only. The host is running PulseAudio
17.0 with its native socket at:

```text
/var/run/xdg/regueiro/pulse/native
```

PipeWire and WirePlumber were also present, but no global audio service was
changed or restarted.

Additional ignored project-local data was created:

- `runtime/app/org.gnome.Decibels`
- `state/apps/org.gnome.Decibels.ini`
- `state/exports/org.gnome.Decibels.list`
- exported Decibels desktop/icon/metainfo files under `exports/share`
- additional Flathub OSTree objects under `downloads/objects`
- `runtime/test-media/audio-test-tone.wav`
- `runtime/chroots/org.gnome.Decibels/var/data/audio-test-tone.wav`

The launcher now creates temporary PulseAudio files only inside the app chroot
when the app metadata declares `sockets=pulseaudio`:

- `runtime/chroots/<app-id>/var/config/pulse/client.conf`
- `runtime/chroots/<app-id>/var/config/pulse/cookie`

Those files are removed during sandbox cleanup. Post-test checks found no
remaining project chroot mounts, no run records, and no temporary PulseAudio
config or cookie files inside the Decibels chroot.

## 2026-08-17 Host FileChooser Portal Follow-up

No persistent system configuration changes were made.

The native host portal stack was inspected only. During document portal
investigation, `vfs.usermount` was temporarily changed from `0` to `1` and
then restored to `0`. No loader, rc, boot, `/etc`, `/usr/local`, or
`/compat/linux` configuration was changed.

Additional project-local source and generated data:

- `src/portal.rs`
- `scripts/portal-bridge.c`
- `target/portal/portal-bridge`
- temporary per-run document grant directories under `runtime/portal/doc`
- temporary per-run private D-Bus directories under
  `/var/run/xdg/regueiro/freebsd-flatpak-poc`

The launcher now starts a private D-Bus session bus and portal bridge per run
when the host session has `DBUS_SESSION_BUS_ADDRESS`. For `FileChooser.OpenFile`,
the bridge reuses the native host picker, creates read-only single-file nullfs
grants for selected regular files, and returns chroot-visible document URIs to
the Linux Flatpak app.

Validation used Decibels to select an MP3 from `/home/regueiro/Downloads`
without granting Decibels blanket Downloads access. The user confirmed Decibels
played the selected host file through the existing PulseAudio bridge.
