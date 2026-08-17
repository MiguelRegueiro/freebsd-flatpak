# Discord investigation

Date: 2026-08-17

Command under test:

```sh
bin/flatpak run com.discordapp.Discord
```

## Result

Discord reached a usable GUI through the existing architecture:

```text
Linux Flatpak payload
-> Linuxulator
-> chroot/nullfs sandbox
-> host Wayland session
```

The user confirmed the Discord login screen appeared and then successfully
logged in.

## First blocker: Chromium shared memory

The initial launch reached the Discord wrapper, then Chromium aborted:

```text
Creating shared memory in /dev/shm/.org.chromium.Chromium.* failed: Operation not supported (95)
Unable to access(W_OK|X_OK) /dev/shm: Permission denied (13)
FATAL ... This is frequently caused by incorrect permissions on /dev/shm
```

The chroot was using the plain devfs view and had no usable Linux-style
`/dev/shm` tmpfs for Chromium/Zypak.

Fix:

- mount a per-run `tmpfs` at `dev/shm`
- use mode `1777`
- keep it owned by the existing `ChrootNullfsBackend` cleanup stack

This is generic for Linux GUI payloads that expect POSIX shared memory under
`/dev/shm`.

## Second blocker: chroot DNS

After `/dev/shm` was fixed, Discord created its Wayland splash/main window,
but Chromium reported:

```text
ERR_INTERNET_DISCONNECTED
```

Direct checks inside the same active chroot showed:

```text
cat /etc/resolv.conf
cat: /etc/resolv.conf: No such file or directory

curl -I --max-time 10 https://discordapp.com/app
curl: (6) Could not resolve host: discordapp.com
```

Host DNS worked at the same time.

Fix:

- replace the old `/etc -> /usr/etc` symlink with a chroot-local `/etc`
  overlay directory
- mirror the Flatpak runtime's `/usr/etc` entries with symlinks
- for apps declaring `shared=network`, refresh project-local copies of host
  `/etc/resolv.conf` and `/etc/hosts` into the overlay before launch
- for apps without `shared=network`, remove those resolver files from the
  overlay

Post-fix checks inside the active chroot:

```text
getent hosts discordapp.com
162.159.133.233 discordapp.com
...

curl -I --max-time 10 https://discordapp.com/app
HTTP/2 301
location: https://discord.com/app
```

Discord's own connectivity probe changed from `res-err.err-internet-disconnected`
to `res-ok`, then Discord connected to `gateway.discord.gg` and completed login.

## Cleanup audit

After the user confirmed login, the test-launched Discord process was stopped
from the terminal. The interrupted launcher left a stale run record and a
subset of the chroot mounts behind, so the existing startup recovery path was
exercised with:

```sh
bin/flatpak list
```

That recovery removed:

- `state/runs/com.discordapp.Discord.ini`
- all mountpoints under `runtime/chroots/com.discordapp.Discord`
- the per-run portal document grant directory

Post-recovery checks showed:

- no `freebsd-flatpak-poc` mountpoints in `mount`
- no Discord, Zypak, private portal bridge, or per-run private D-Bus processes
- no run records in `state/runs`
- only the base `runtime/portal/doc` directory remained

The initial crash reproduction generated an untracked `flatpak.core`, which
was deleted after explicit approval.

A later retry exposed one more cleanup bug: Electron/Chromium utility and
renderer children can survive after the recorded launcher/child PIDs are gone,
remain rooted in the stale chroot, and keep `devfs`, `tmpfs /dev/shm`, and
`linprocfs` busy. Startup recovery now scans stale chroot mount holders, filters
them to processes whose `procstat -f` root/jail/cwd is the stale chroot, and
terminates those holders before unmounting.

## Known remaining warnings

These warnings did not block the confirmed GUI/login path:

- missing system bus at `/run/dbus/system_bus_socket`
- missing `org.freedesktop.portal.Flatpak`
- missing `/run/host/font-dirs.xml`
- EGL/GPU initialization failures, followed by Chromium software fallback
- `OpenURI` portal interface missing from the minimal private portal bridge
- video capture context provider failures

No VM, Linuxulator fork, generic syscall layer, boxrun work, GPU work, or
Discord-specific path hacks were added.
