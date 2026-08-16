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
