# Generic Payload Launcher

Date: 2026-08-16

## Milestone

The Calculator-specific launch path was refactored into a generic Rust runner:

```sh
cargo run -- run <app-id>
```

The runner preserves the V1 architecture:

```text
Rust launcher
  -> FreeBSD chroot/nullfs sandbox backend
  -> Linuxulator dynamic loader
  -> unmodified Linux Flatpak app payload
  -> host Wayland socket
```

## Resolution Model

Given an app id, the default lookup is:

- app checkout: `runtime/app/<app-id>`
- app metadata: `runtime/app/<app-id>/metadata`
- runtime ref: `[Application] runtime` from metadata
- entry command: `[Application] command` from metadata
- runtime checkout: `runtime/<runtime-name>-<branch>`

The `run` command also accepts explicit overrides:

```sh
cargo run -- run <app-id> --app-dir <path> --runtime-dir <path> --entry <executable>
```

## Sandbox Backend

The backend is intentionally abstracted behind a Rust trait so the current
`chroot`/nullfs implementation can later be replaced by boxrun, jail, or
another FreeBSD-native backend.

For each app launch the backend creates:

- `runtime/chroots/<app-id>/usr` as read-only nullfs from the Flatpak runtime
- `runtime/chroots/<app-id>/app` as read-only nullfs from the Flatpak app
- `runtime/chroots/<app-id>/run/user/<uid>` as nullfs from host
  `XDG_RUNTIME_DIR`
- `runtime/chroots/<app-id>/tmp` as nullfs from host `/tmp`
- `devfs`, `linprocfs`, and `linsysfs` inside the chroot

The launcher waits for the app process to exit and then unmounts the sandbox
mounts in reverse order.

## Validated Apps

### org.gnome.Calculator

- Flatpak ref: `app/org.gnome.Calculator/x86_64/stable`
- Runtime: `org.gnome.Platform/x86_64/50`
- Command from metadata: `gnome-calculator`
- Launch command:
  `cargo run -- run org.gnome.Calculator`
- User visually confirmed the window appeared and worked through the generic
  runner.

### org.gnome.TextEditor

- Flatpak ref: `app/org.gnome.TextEditor/x86_64/stable`
- Commit:
  `37a78f29e6b6f6c01ec1de618f25a07502e1ebdc54ef3d91978e37ef402279ae`
- Runtime: `org.gnome.Platform/x86_64/50`
- Command from metadata: `gnome-text-editor`
- Checkout result: 197 directories, 351 files
- Launch command:
  `cargo run -- run org.gnome.TextEditor`
- Process evidence showed:
  - root/cwd/jail under `runtime/chroots/org.gnome.TextEditor`
  - app text mapping from `runtime/app/org.gnome.TextEditor/files/bin`
  - runtime loader mapping from `runtime/org.gnome.Platform-50/files/lib`
  - live file descriptor to `/var/run/xdg/regueiro/wayland-1`
  - live file descriptor to `/tmp/dbus-1Xv4JVVacF`
- User visually confirmed the window appeared and worked through the same
  generic runner.

## Known Differences Between The Two Flatpaks

- Both apps used the same runtime: `org.gnome.Platform/x86_64/50`.
- The only required app-specific values were data values from Flatpak metadata:
  app id and command.
- Text Editor warned about an unknown passwd entry for uid `1001`; Calculator
  did not surface that warning. A generated `/etc/passwd` inside the chroot is
  a likely next small fix.
- Calculator attempted DNS for currency rates; Text Editor did not need network
  for startup.

## Remaining Hardcoded Items

- The backend is currently chroot/nullfs only.
- The Linux dynamic loader path is `/lib64/ld-linux-x86-64.so.2`.
- The environment profile is GTK-oriented and still manually curated.
- App commands are not parsed as shell commands; V1.1 accepts a single
  executable name or absolute path.
- The host `/tmp` mount is still required for this session's D-Bus address.
- Fontconfig still warns about missing `/run/host/font-dirs.xml`.
- AT-SPI still tries the host absolute path and is not cleanly mapped yet.
- Cleanup runs after normal app exit and after setup/launch errors. Signal-time
  cleanup is not implemented yet.
