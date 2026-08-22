# org.gnome.TextEditor cold Ethernet benchmark — 2026-08-22

Host: FreeBSD 15.1 amd64. Default route: `ue0` (Ethernet). Remote: Flathub.
The benchmark used a new temporary HOME plus isolated project and XDG roots.
It completed installation and desktop export without touching the real user
data directories.

Application commit:
`37a78f29e6b6f6c01ec1de618f25a07502e1ebdc54ef3d91978e37ef402279ae`.
Runtime: `org.gnome.Platform/x86_64/50` at
`a8da766dd0273a67539d2b98358ed6809d8e729280baf63428108837135229d3`.

| Phase | Time |
| --- | ---: |
| Resolution | 1.419 s |
| libostree pull | 42.946 s |
| Checkout | 6.706 s |
| Desktop export | 0.012 s |
| Total | 54.920 s |

The app and runtime used 13 static-delta parts (337.5 MiB); the GL extension
used one (138.2 MiB). `/usr/bin/time` measured 54.98 s wall time. The isolated
lifecycle validation then confirmed `list`, `run` resolution, `uninstall`, an
empty subsequent `list`, and zero remaining launcher projections.

Re-run the measurement with:

```sh
LD_LIBRARY_PATH="$PWD/target/vendor-ostree/prefix/lib" \
  ./scripts/benchmark-text-editor.sh cold-ethernet target/release/flatpak
```
