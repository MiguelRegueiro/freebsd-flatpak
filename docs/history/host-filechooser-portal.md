# Host FileChooser Portal Milestone

Date: 2026-08-17

## Host Portal Setup

The FreeBSD desktop session was inspected before implementation:

- Session bus:
  `unix:path=/tmp/dbus-1Xv4JVVacF,guid=a5dc0afa62efaecc72d905a56a81ae66`
- Runtime dir: `/var/run/xdg/regueiro`
- Wayland display: `wayland-1`
- Host portal processes were already running:
  - `/usr/local/libexec/xdg-desktop-portal`
  - `/usr/local/libexec/xdg-desktop-portal-gtk`
  - `/usr/local/libexec/xdg-desktop-portal-hyprland`
  - `/usr/local/libexec/xdg-permission-store`
- Installed portal packages included `xdg-desktop-portal`,
  `xdg-desktop-portal-gtk`, `xdg-desktop-portal-hyprland`, and `libportal`.

The host `org.freedesktop.portal.Desktop` service exposes
`org.freedesktop.portal.FileChooser` version 4, which is the interface used for
the native picker UI.

## Native Document Portal Blocker

The native host document portal could not be used directly for this V1:

- `org.freedesktop.portal.Documents.GetMountPoint` timed out or disconnected
  through the normal session bus.
- Running `/usr/local/libexec/xdg-document-portal -v -r` failed first with
  `fuse: unknown option(s): -o auto_unmount`.
- A temporary preload shim that stripped unsupported FUSE options got the
  process farther, but mounting under `/var/run/xdg/regueiro/doc` still failed.
- `vfs.usermount` was temporarily changed from `0` to `1` during investigation
  and then restored to `0`. This was not made persistent.

Because the native document portal was not usable, the V1 implements the
smallest bridge around the working native FileChooser UI.

## Launcher Behavior

Each app run now starts a project-controlled portal path:

- a private per-run `dbus-daemon` socket under
  `/var/run/xdg/regueiro/freebsd-flatpak/<app>-<pid>/bus`;
- `compatibility_helpers/portal-bridge.c`, connected to that private bus and to the real host
  session bus;
- a project-local document source directory under
  `runtime/portal/doc/<app>-<pid>`;
- a chroot-visible document mount target at
  `runtime/chroots/<app-id>/run/user/<uid>/doc`.

The launched Linux Flatpak gets:

```text
GTK_USE_PORTAL=1
DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/<uid>/freebsd-flatpak/<app>-<pid>/bus
```

The bridge owns `org.freedesktop.portal.Desktop` and
`org.freedesktop.portal.Documents` on the private bus. For
`FileChooser.OpenFile`, it forwards the request to the native host
`xdg-desktop-portal`, lets the normal desktop file chooser run, then rewrites
returned host `file://` URIs to `/run/user/<uid>/doc/...` document URIs.

For each selected regular file, the bridge creates a read-only single-file
nullfs mount directly on the chroot-visible document path. Directory selections
are exported recursively at the same kind of document path with read-write
access. This direct target mount matters on FreeBSD: a nested mount created only
under the source document directory did not propagate through the
already-mounted parent nullfs view and appeared as a placeholder inside the
chroot. The bridge applies the same direct mapping in both lifecycle orders:
existing grants are mounted when a sandbox registers, and newly created grants
are mounted into every already-registered sandbox before the FileChooser
response is returned.

FileChooser grants are recorded under the managed data root, outside the
ephemeral runtime portal tree. A later bridge process restores the same document
ID and mounts it into each new sandbox instance, so an application can safely
persist `/run/user/<uid>/doc/<id>/<name>`. When that path is sent back as a
subsequent chooser's `current_folder`, the private bridge resolves it to the
host path before forwarding the request. Host paths that cannot be granted are
dropped rather than returned unchanged to the sandbox.

Document IDs are opaque, collision-checked tokens encoded from 128 random bits.
The chooser reuses the persistent grant for the same canonical host path and
entry type instead of allocating a new ID.

## Validation

Test app:

```sh
bin/flatpak run org.gnome.Decibels
```

Test workflow:

1. Click Open in Decibels.
2. Select an MP3 from `/home/regueiro/Downloads`.
3. The native desktop file picker returns the real host file URI.
4. The bridge creates a read-only single-file nullfs grant under the Decibels
   chroot document path.
5. The bridge emits `org.freedesktop.portal.Request.Response` directly to the
   Decibels D-Bus peer with the rewritten document URI.
6. Decibels opens the document URI and plays through the existing PulseAudio
   bridge.

The user visually confirmed the picker workflow and audibly confirmed playback.

Runtime checks while Decibels was playing showed:

- the selected MP3 visible inside the Linux chroot under `/run/user/1001/doc`;
- the file size matched the host MP3 instead of the earlier 0-byte placeholder;
- the first bytes were an ID3 header;
- the file was mounted read-only with nullfs;
- PulseAudio sink inputs were present.

## Cleanup

The launcher keeps portal resources per-run. Normal cleanup terminates the
bridge, unmounts selected-file grants, removes the project document directory,
terminates the private `dbus-daemon`, removes the private bus directory, and
then lets the existing sandbox cleanup unmount the parent document directory and
the rest of the chroot.

Startup recovery also scans `runtime/portal/doc` and
`$XDG_RUNTIME_DIR/freebsd-flatpak` for stale inactive runs and removes
project-owned leftovers where possible.

## Remaining Gaps

FileChooser support currently handles opening existing regular files and
directories. It does not implement save portals, broad document portal
compatibility, GVFS peer sockets, or other portal interfaces beyond the minimal
GTK startup surface.

Relevant upstream references:

- https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.FileChooser.html
- https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Documents.html
- https://flatpak.github.io/xdg-desktop-portal/docs/documents-and-fuse.html
