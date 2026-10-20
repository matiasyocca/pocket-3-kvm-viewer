# Pocket 3 KVM Viewer

A Linux viewer for the optional GPD Pocket 3 KVM module's HDMI input. See the target computer's video in a window or full screen, with touchscreen-friendly controls.

The module itself forwards the Pocket 3's keyboard and touchpad when its USB cable is connected to the target. The viewer does not inject input or require a privileged helper.

## Install and launch

You need the optional KVM module, a Linux graphical session, and permission to read its `/dev/videoN` capture device. Windows and macOS are not supported.

### Linux release binary

Download the x86_64 archive and its matching checksum from [GitHub Releases](https://github.com/matiasyocca/pocket-3-kvm-viewer/releases). No aarch64 release binary is provided yet.

The archive is **not self-contained**: install GTK4 4.10+, GStreamer core/base libraries, and the GStreamer base/good and ALSA runtime plugins. On Ubuntu 24.04:

```sh
sudo apt install libgtk-4-1 libgstreamer1.0-0 libgstreamer-plugins-base1.0-0 \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-alsa
```

In the download directory, substitute the actual archive name:

```sh
sha256sum -c <archive-name>.sha256
tar -xzf <archive-name>.tar.gz
./pocket3-kvm-viewer
```

Release binaries are built on Ubuntu 24.04 and are not guaranteed to work on older distributions. On other distributions, install equivalent runtime packages and check library compatibility, or [build from source](docs/DEVELOPMENT.md#build-from-source). No separate GTK4 GStreamer sink plugin is needed.

### Nix and NixOS

From a checkout with flakes enabled:

```sh
nix run .
```

To build the wrapped package: `nix build .#pocket3-kvm-viewer`. It supplies GTK/GStreamer runtime dependencies and installs the `pocket3-kvm-viewer` command and a “Pocket 3 KVM Viewer” desktop entry. Use this package on NixOS rather than the conventional Linux release archive.

To install through your NixOS flake:

```nix
# Add to your flake inputs:
pocket3-kvm-viewer.url = "github:matiasyocca/pocket-3-kvm-viewer";

# Pass inputs to your nixosSystem modules if not already done:
specialArgs = { inherit inputs; };

# In a module taking { pkgs, inputs, ... }:
environment.systemPackages = [
  inputs.pocket3-kvm-viewer.packages.${pkgs.stdenv.hostPlatform.system}.default
];
```

The flake declares x86_64-linux and aarch64-linux; only x86_64-linux has been build-tested. Source-build requirements and optional build features are in the [development guide](docs/DEVELOPMENT.md).

## Connect the target

1. Install the optional KVM module.
2. Connect the target's HDMI OUT to the module's HDMI IN—not the Pocket 3's own HDMI OUT.
3. Launch the viewer. It starts automatically when exactly one supported module is found, or waits for you to connect it.
4. Connect the target's USB port to the module's USB IN for hardware keyboard/touchpad forwarding. Use the Pocket 3's touchscreen to operate the viewer while its keyboard and touchpad control the target.

## Controls

- **Stop / Start:** Stop releases the capture device and disables automatic reconnect until you press Start. Start searches again and resumes capture.
- **Full screen:** Use the overlay button or double-click the video with the primary pointer button.
- **Hidden controls:** Controls and the pointer hide after three seconds without pointer movement over the viewer. Move the pointer or touch the screen to reveal them. If hidden, the first touch reveals the controls; a second touch can activate a button.
- **Reconnect:** With automatic capture enabled, unplugging the module stops capture; reconnecting it starts capture again. If device monitoring is unavailable, use Start to retry.

Video preserves its aspect ratio instead of cropping the target's screen. Errors and connection messages appear over the video when needed.

The Nix package pauses capture when GTK/the compositor reports the window hidden, suspended, or minimized, then resumes when shown. It retains the device and last image while paused; use Stop to release the device. Losing focus alone does not pause video. Compositors that do not report occlusion cannot trigger occlusion-based suspension. The release archive and ordinary Cargo builds do not enable this optional behavior.

## HDMI audio

Optional audio monitoring is available for the supported USB `3188:1000` KVM module. It plays the module's captured HDMI audio through your computer's normal audio output. Start video first, then tap the speaker icon to unmute.

Each launch starts muted at 25% viewer volume. The slider adjusts only this viewer, from 0 to 100%; moving it above zero unmutes, and zero mutes. Muting preserves a nonzero volume; unmuting from zero restores 25%. Your choice is retained across Stop/Start and reconnects until you close the app.

Audio stops on mute, Stop, capture failure, disconnect and close. In builds with background suspension enabled, audio also stops while video is suspended and resumes with video. Focus loss alone does not stop it.

A warning icon beside the slider means audio is unavailable; its tooltip explains why. Video continues independently. If the module's audio interface appears late, the viewer retries briefly; otherwise mute then unmute to retry. A different recorder or the audio server may already be holding the module's capture device. Do not change your system microphone or disable your audio server to work around this.

Audio uses the module's explicitly associated ALSA capture interface, never the default microphone. Other video devices selected with `--device` do not automatically gain audio support. Monitoring is not a synchronized A/V player; some audio/video offset may be noticeable.

## Sleep prevention

While capture is actually playing and the viewer has focus, it asks systemd-logind to prevent idle/sleep. Losing focus, stopping capture, a capture error, or closing the viewer releases the request. If logind is unavailable or denies it, video still works and a warning is printed to stderr.

This only affects power management that honors logind inhibitors: forced suspend, some lid policies, and compositor-specific idle actions may still occur. The viewer cannot verify that an HDMI signal is present; a running capture module without a working target signal may still inhibit sleep.

## Troubleshooting

- **No device or multiple devices:** The supported module is detected automatically. For a different module revision or multiple capture devices, select a node explicitly:

  ```sh
  pocket3-kvm-viewer --device /dev/video2
  ```

  Replace the path with your actual capture node. Video-node numbers can change; `v4l2-ctl --list-devices` and `v4l2-ctl -d /dev/videoX --all` help identify it.

- **Capture fails:** Check that the selected node is readable by your user and is not held by another viewer. Do not run the GUI as root just to access a video node. Run `GST_DEBUG=2 pocket3-kvm-viewer` from a terminal for diagnostics; for an extracted archive, use `GST_DEBUG=2 ./pocket3-kvm-viewer`.
- **Sleep prevention is unexpected:** Check `systemd-inhibit --list` while capture is playing and the viewer is focused. Stop capture to release the viewer's request.

## Limitations

No recording, automatic application launch on hot-plug, firmware controls, remote/IP KVM, or software input forwarding. Alternate capture nodes require `--device`.

Display latency, total power consumption, touchscreen behavior, and USB input forwarding have not been fully validated. See the [hardware validation notes](docs/HARDWARE.md) for the distinction between observed results and outstanding checks.

## Contributor documentation

- [Development, build features, tests, and releases](docs/DEVELOPMENT.md)
- [Architecture navigation](docs/ARCHITECTURE.md)
- [Hardware evidence and manual validation](docs/HARDWARE.md)
- [Performance measurements and optimization decisions](docs/PERFORMANCE.md)

Implementation behavior and invariants live in the code and tests; this README is the user guide.
