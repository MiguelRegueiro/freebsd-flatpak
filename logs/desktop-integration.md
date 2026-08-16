# Desktop Integration Milestone

Date: 2026-08-16

## Milestone

`flatpak install` and `flatpak update` now export app desktop integration data
from the real Flatpak payload under:

```sh
exports/share
```

The exported files are tracked per app with manifests under:

```sh
state/exports/<app-id>.list
```

`flatpak uninstall` removes the files listed in that app's export manifest and
refreshes local desktop/icon caches afterward.

## Tested Apps

The export path was tested with:

```sh
bin/flatpak install org.gnome.Calculator
bin/flatpak install org.gnome.TextEditor
bin/flatpak install org.gnome.Characters
bin/flatpak update
bin/flatpak uninstall org.gnome.Calculator
bin/flatpak install org.gnome.Calculator
```

Final installed state contains:

- `org.gnome.Calculator`
- `org.gnome.TextEditor`
- `org.gnome.Characters`

Each app exports one `.desktop` file, scalable/symbolic hicolor icons, and
metainfo. The generated desktop files passed `desktop-file-validate`.

## Intentional V1 Differences

- `Exec=` is rewritten to call the project-local launcher:
  `/home/regueiro/freebsd-flatpak-poc/bin/flatpak run <app-id>`.
- Original desktop-action arguments and field codes are preserved after `--`.
- `DBusActivatable=true` is rewritten to `false` so launchers use `Exec=`.
- Exported D-Bus service files and GNOME Shell search providers are skipped
  because they would require host-side D-Bus activation support.
- MIME associations are preserved in desktop files, but file paths passed by a
  launcher may still be inaccessible inside the chroot unless separately exposed.
