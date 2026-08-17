# Host Audio Milestone

Date: 2026-08-17

## Host Audio Setup

The host audio stack was inspected before changing the launcher:

- FreeBSD exposes OSS devices through `/dev/dsp*`; `/dev/sndstat` reports
  Realtek ALC295 playback/recording devices.
- The active desktop audio server is PulseAudio 17.0.
- `pactl info` reports the server socket as
  `/var/run/xdg/regueiro/pulse/native`.
- PipeWire and WirePlumber are running, and `/var/run/xdg/regueiro/pipewire-0`
  exists, but the tested Flatpak app requested PulseAudio.
- The PulseAudio cookie exists at `/home/regueiro/.config/pulse/cookie`.

No host audio service was replaced or reconfigured.

## Launcher Behavior

The launcher now parses each installed app's Flatpak metadata:

```ini
[Context]
sockets=...
```

If an app declares `pulseaudio`, the existing `ChrootNullfsBackend` prepares a
per-run PulseAudio bridge:

- The already-mounted host `XDG_RUNTIME_DIR` exposes the host PulseAudio socket
  inside the chroot at `/run/user/<uid>/pulse/native`.
- The launcher sets `PULSE_SERVER=unix:/run/user/<uid>/pulse/native`.
- If a host PulseAudio cookie is available, it is copied into the chroot at
  `/var/config/pulse/cookie` and `PULSE_COOKIE` points at that sandbox path.
- The launcher writes `/var/config/pulse/client.conf` with the same default
  server and `autospawn = no`.
- The copied cookie and client config are removed during sandbox cleanup.

Apps that do not declare `pulseaudio` receive no PulseAudio environment from
this layer. The `flatpak permissions <app-id>` inspector now prints both
declared socket permissions and the resolved audio bridge.

PipeWire-native socket detection is present as a future hook, but no
PipeWire-native playback path has been validated.

## Validation

`org.gnome.Decibels` was installed from Flathub because its metadata declares:

```ini
sockets=x11;wayland;pulseaudio;fallback-x11;
```

A three-second WAV test tone was generated under the project:

```text
runtime/test-media/audio-test-tone.wav
```

The test file was also copied into Decibels' sandbox data directory so the
audio test would not require broad host filesystem access:

```text
runtime/chroots/org.gnome.Decibels/var/data/audio-test-tone.wav
```

Launch command:

```sh
bin/flatpak run org.gnome.Decibels -- /var/data/audio-test-tone.wav
```

Runtime inspection showed a PulseAudio client connection and sink inputs while
Decibels was running. The user audibly confirmed that the test tone played
through the normal FreeBSD desktop audio output.

After the run was interrupted and cleaned up, checks found no remaining mounts
under `runtime/chroots`, no run records under `state/runs`, and no temporary
PulseAudio config or cookie files left inside the Decibels chroot.

## Remaining Gaps

Decibels does not declare filesystem permissions, so opening arbitrary music
from `~/Downloads` through its file picker still needs a later portal or
project-local per-app filesystem override layer. This was deliberately left out
of the audio milestone.

Audio support has only been proven for PulseAudio. GPU, camera, portals,
boxrun, Discord, and broader compatibility remain out of scope.
