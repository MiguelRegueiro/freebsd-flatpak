# FreeBSD Flatpak POC

V1 goal: launch an unmodified Linux Flatpak application through FreeBSD
Linuxulator, using FreeBSD-native sandbox/runtime setup instead of a VM.

This workspace is intentionally self-contained. Downloads, extracted Flatpak
runtime/app data, scripts, logs, transient mount targets, and host-side Rust
code live here unless a system-level dependency is unavoidable.

## Current Constraints

- No bhyve, QEMU, or VM.
- Do not fork or significantly modify Linuxulator.
- Do not implement generic Linux syscall compatibility.
- Prefer replacing Flatpak's Linux-specific sandbox layer with FreeBSD
  mechanisms.
- Use existing Linuxulator for Linux ELF execution.
- Keep persistent system changes minimal, explicit, and reversible.

