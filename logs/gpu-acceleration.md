# GPU acceleration investigation

Date: 2026-08-17

Target app: `io.github.zen_browser.zen`

## Host graphics path

- FreeBSD: `15.1-RELEASE`
- Kernel modules relevant to the test: `i915kms`, `drm`, `dmabuf`, `linux`, `linux64`,
  `linprocfs`, `linsysfs`, `nullfs`
- Primary GPU: Intel `CometLake-U GT2 [UHD Graphics]`
- DRM nodes:
  - `/dev/dri/card0 -> ../drm/0`
  - `/dev/dri/renderD128 -> ../drm/128`
- DRM PCI identity:
  - `hw.dri.0.busid=pci:0000:00:02.0`
  - `dev.drm.0.PCI_ID=8086:9b41`
  - `dev.drm.128.PCI_ID=8086:9b41`

## Flatpak runtime graphics path

Zen uses `org.freedesktop.Platform/x86_64/24.08`.

That runtime declares:

- `[Extension org.freedesktop.Platform.GL]`
- `directory = lib/x86_64-linux-gnu/GL`
- `versions = 24.08;24.08extra;1.4`
- `download-if = active-gl-driver`
- `enable-if = active-gl-driver`

The matching extension is:

- `runtime/org.freedesktop.Platform.GL.default/x86_64/24.08`

GNOME 50 runtimes declare the same GL extension, but with `versions = 25.08;25.08-extra;1.4`,
so extension resolution must use the metadata `versions` field rather than the runtime branch.

## Failure before the fix

Zen launched and rendered pages, but Firefox/Zen glxtest logged:

- `glxtest: libEGL no display`
- `glxtest: EGL test failed`
- `No GPUs detected via PCI`

Manual probing showed two missing pieces:

1. The runtime had no mounted `org.freedesktop.Platform.GL.default` extension, so Mesa DRI drivers
   such as `iris_dri.so` were absent from the runtime library search path.
2. FreeBSD `linsysfs` exposed some DRM information, but not the Linux render-node paths expected by
   Mesa/libdrm:
   - `/sys/class/drm/renderD128`
   - `/sys/dev/char/226:128`
   - `/sys/bus/pci/devices/0000:00:02.0/subsystem`

With the GL extension but incomplete sysfs, glxtest fell back to llvmpipe:

```text
DRI_DRIVER
swrast
VENDOR
Mesa
RENDERER
llvmpipe (LLVM 19.1.7, 256 bits)
MESA_ACCELERATED
FALSE
TEST_TYPE
EGL
```

## Implemented bridge

Added a generic host graphics layer:

- resolves and caches the Flatpak GL extension from runtime metadata;
- mounts the extension read-only at `/usr/lib/x86_64-linux-gnu/GL/default`;
- prepends the extension library path to `LD_LIBRARY_PATH`;
- sets Mesa lookup paths for DRI, GBM, EGL vendor files, and EGL external platform config;
- creates a per-run project-local Linux DRM sysfs overlay from host DRM/PCI data;
- mounts those overlays read-only on:
  - `/sys/bus`
  - `/sys/dev/char`
  - `/sys/class/drm`
- removes the generated sysfs overlay tree after unmount.

No host graphics stack settings were changed.

## Low-level result

Inside the new Zen sandbox, `glxtest` now reports the real Intel renderer:

```text
PCI_VENDOR_ID
0x8086
PCI_DEVICE_ID
0x9b41
DRI_DRIVER
iris
VENDOR
Intel
RENDERER
Mesa Intel(R) UHD Graphics (CML GT2)
DRM_RENDERDEVICE
/dev/dri/renderD128
TEST_TYPE
EGL
```

GUI/browser smoothness and `about:support` graphics status are pending user confirmation.
