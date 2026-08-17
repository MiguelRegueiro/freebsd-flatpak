# Zen Browser compatibility test

Date: 2026-08-17

## Target

- App: `io.github.zen_browser.zen`
- Runtime: `org.freedesktop.Platform/x86_64/24.08`
- Base app: `app/org.mozilla.firefox.BaseApp/x86_64/24.08`
- Command from metadata: `launch-script.sh`
- Wrapper behavior: `exec /app/zen/zen "$@"`

## Result

Zen Browser launched through the existing architecture:

```text
Linux Flatpak payload
-> Linuxulator
-> FreeBSD chroot/nullfs sandbox
-> host Wayland session
```

The user confirmed:

- the initial setup screen appeared as a normal Hyprland window
- keyboard and mouse input worked
- the host cursor theme appeared correctly in the Zen window
- Google search worked
- YouTube video and audio playback worked through the existing PulseAudio bridge

No Zen-specific compatibility code was added.

## Sandbox Inputs

Metadata-derived permissions exposed by the current launcher:

- `xdg-download` read-write through the filesystem permissions layer
- host PulseAudio socket and cookie through the audio layer
- host Wayland runtime directory
- project private portal bridge and document grant directory
- host cursor theme mounted read-only under `/run/host/share/icons`

## Observed Graphics Warnings

Zen worked, but felt somewhat slow. The log showed GPU/EGL probing failures:

```text
glxtest: libEGL no display
glxtest: EGL test failed
No GPUs detected via PCI
RenderCompositorSWGL failed mapping default framebuffer
```

This indicates Firefox/Zen fell back to software rendering. GPU acceleration was
not changed during this compatibility test.

## Cleanup

After the user closed Zen:

- no Zen/freebsd-flatpak-poc mountpoints remained
- no run records remained under `state/runs`
- no Zen, portal bridge, zypak, socat, or sandbox helper processes remained

Post-test checks:

- `cargo test`: 22 passed
- `git diff --check`: clean

## Core File Note

An old untracked `flatpak.core` from an earlier Discord run was inspected with
LLDB. It showed the Rust launcher aborting during cleanup after that old
experiment, not a new Zen or browser runtime crash. The core was removed and was
not committed.
