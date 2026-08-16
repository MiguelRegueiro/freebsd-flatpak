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
