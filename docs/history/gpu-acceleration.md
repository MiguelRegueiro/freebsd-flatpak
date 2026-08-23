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

## Correctness and performance follow-up

Follow-up testing compared Zen with native FreeBSD Firefox:

- Native Firefox `glxtest` reports `DRI_DRIVER=iris`, Intel UHD Graphics, EGL, and
  `/dev/dri/renderD128`.
- Zen Flatpak `glxtest` reports the same Intel `iris` renderer through Linuxulator after the GL
  extension and sysfs bridge are present.
- Native Firefox carries `MOZ_ENABLE_WAYLAND=1`; both native Firefox and Zen run against the host
  Hyprland Wayland socket.
- The remaining `libEGL`/Zink warnings occur during Mesa probing. They do not by themselves prove a
  software-rendering fallback, because the explicit Firefox glxtest path now succeeds with Intel
  iris.
- Zen still emitted `RenderCompositorSWGL failed mapping default framebuffer` during a browser run,
  so WebRender/compositor status must be checked in `about:support` rather than inferred from
  glxtest alone.

Two additional generic sandbox omissions were found:

1. The Freedesktop runtime's fontconfig includes `/run/host/font-dirs.xml`; the sandbox did not
   provide that file, causing repeated fontconfig errors. The launcher now creates a sandbox-local
   `/run/host/font-dirs.xml`, exposes host font directories read-only when present, and keeps
   fontconfig caches sandbox-local.
2. Firefox's Linux memfd read-only duplication path opens `/proc/self/fd/N`. In the sandbox,
   `linprocfs` pointed `/proc/self/fd` at `/dev/fd`, but plain `devfs` exposed only `0`, `1`, and
   `2`. Mounting `fdescfs` at `/dev/fd` made `/proc/self/fd/N` usable and removed the
   `read-only dup failed ... not using memfd` warning.

Hardware video decoding is separate from GPU rendering. Native Firefox loaded
`/usr/local/lib/dri/iHD_drv_video.so` in its RDD process during video playback. Zen loaded `libva`
but no Intel VAAPI driver before the fix, because `org.freedesktop.Platform.VAAPI.Intel` was not
mounted. The launcher now resolves and mounts that runtime extension for Intel DRM hosts and adds
its library/driver paths to the sandbox environment.

Additional follow-up:

- The `org.freedesktop.Platform.ffmpeg-full` app extension was present in Zen metadata but not
  mounted under `/app/lib/ffmpeg`. The launcher now resolves app-declared
  `org.freedesktop.Platform.ffmpeg-full` extensions, mounts them read-only at the app-declared
  directory, and prepends their `add-ld-path` to `LD_LIBRARY_PATH`. Zen media decoder processes now
  load `libavcodec`, `libavutil`, `libx264`, and `libx265` from the real Flatpak extension.
- The remaining Zink startup failure was caused by Vulkan ICD discovery. The GL extension contained
  the Intel ICD, but the sandbox did not point Vulkan at it. The graphics bridge now selects a
  vendor-appropriate ICD from the detected DRM PCI vendor and sets `VK_DRIVER_FILES` and
  `VK_ICD_FILENAMES`. After this, the `ZINK: vkCreateInstance failed
  (VK_ERROR_INCOMPATIBLE_DRIVER)` and `egl: failed to create dri2 screen` warnings disappeared.
- Zen metadata declares `MESA_SHADER_CACHE_DIR=$XDG_RUNTIME_DIR/app/$FLATPAK_ID/cache/mesa_shader_cache_db`.
  The launcher previously ignored `[Environment]`, so that cache path was absent. The sandbox now
  applies Flatpak metadata environment entries with `$VAR`/`${VAR}` expansion and creates declared
  app-scoped runtime directories. The fresh Zen run has `MESA_SHADER_CACHE_DIR` expanded and Mesa is
  writing cache files under the host runtime directory.
- The only startup Mesa warnings left in the latest run are:
  - `libEGL warning: failed to get driver name for fd -1`
  - `libEGL warning: MESA-LOADER: failed to retrieve device information`
  These occur during probing. They are not currently correlated with a software renderer fallback,
  because the browser process has the Flatpak Mesa GL extension, `libEGL_mesa`, `libgbm`,
  `libdrm_intel`, and `libgallium` mapped.
- VAAPI is available inside the same mounted Zen sandbox: `/app/zen/vaapitest -d
  /dev/dri/renderD128` reports `VAAPI_SUPPORTED TRUE`. During YouTube playback, Zen still has not
  mapped `iHD_drv_video.so` in its media decoder process. That points to Firefox/Zen's media decode
  selection or sandboxing, not to a missing FreeBSD DRM device or missing Flatpak VAAPI extension.
- Native Firefox is much newer than this Zen Flatpak: native Firefox is `154.0` build
  `20260813082423`, while Zen is Gecko `134.0` build `20250110005355`. Any final performance
  comparison has to account for browser-version differences as well as the Linuxulator/sandbox
  bridge.
