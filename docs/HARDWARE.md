# Hardware validation

[User guide](../README.md) · [Development](DEVELOPMENT.md) · [Performance evidence](PERFORMANCE.md)

These notes preserve the partial observations previously recorded in the README. They are not a fresh validation run or a guarantee for every module revision. `/dev/videoN` numbering is not stable; substitute the actual node in commands.

## Previously observed hardware

On the implementation host:

- DMI reported vendor GPD and product `G1621-02`, with a Wayland/Hyprland session.
- `v4l2-ctl --list-devices` listed `HDMI Capture: HDMI Capture` at `/dev/video2`, plus a metadata node at `/dev/video3`.
- Sysfs reported USB ID `3188:1000`, V4L2 index 0 for `/dev/video2`, and index 1 for `/dev/video3`.
- The hardware discovery test passed with `/dev/video2`.
- The capture node advertised YUYV 1920×1080 at 30 or 60 fps. A 10-buffer GStreamer V4L2-to-fakesink capture completed without error.
- The native viewer auto-opened `/dev/video2`; GStreamer negotiated YUY2 1920×1080 60/1, the GTK sink created frames repeatedly, and a scoped screenshot showed the target NixOS boot console in the video pane with overlay controls.

Later rendering, visibility-return, and resource-retirement evidence is preserved in [PERFORMANCE.md](PERFORMANCE.md) and its linked artifacts. Negotiated caps alone do not establish actual throughput, visible output, or latency. The performance report records sink-delivery measurements separately.

## Opt-in integration checks

Run in the provisioned development environment from the repository root. Close other viewer instances before capture tests; the video node is exclusive and another viewer's inhibitor can affect the logind assertion.

On a logind host with `systemd-inhibit` installed:

```sh
cargo test --locked focused_capture_holds_and_releases_a_logind_lock -- --ignored
```

With the supported module attached, replace `/dev/video2`:

```sh
KVM_EXPECTED_DEVICE=/dev/video2 cargo test --locked finds_attached_kvm_capture_node -- --ignored
```

With GTK 4.20+ and an available graphical display:

```sh
cargo test --locked --features native-yuv capture::tests::sink_accepts_native_yuy2 -- --ignored --exact
```

The native-YUV test checks offered sink caps, not target content, frame delivery, color correctness, or latency. Hardware-free directory-monitor and synthetic pipeline fixtures exercise event/lifecycle logic, but do not replace physical unplug/replug testing. Read each ignored test's prerequisites before running it; do not indiscriminately enable every ignored diagnostic.

## Manual acceptance checklist

These are checks to perform, not declarations that they have all passed:

- Open the GUI with the real target connected. Check live BIOS and desktop video, aspect ratio, full-screen entry/exit, and return to windowed mode.
- Test hardware keyboard/touchpad switching to the target and back. Test touchscreen controls, including revealing hidden controls and activating them with a subsequent touch.
- Check `systemd-inhibit --list` while focused with Playing capture, after focus loss, after Stop, and after closing the viewer. Exercise a capture error and verify resource/inhibitor release.
- Launch with the module unplugged, attach it, and confirm automatic capture. Unplug/replug during capture and confirm reconnection. Press Stop, unplug/replug, and confirm it remains stopped until Start.
- With `background-throttle` enabled, hide/minimize and return to the viewer. Confirm video resumes; visible but unfocused video must remain live. Physically unplug/replug while suspended and check recovery separately.
- Measure delivered frames and negotiated formats on the target host rather than promising a fixed frame rate. Measure end-to-end display latency and total power separately from process CPU.

## Outstanding validation boundaries

Physical suspended unplug/replug, touchscreen behavior, USB input forwarding, full color-chart correctness, total power, and first-fresh-HDMI/panel latency have not been fully validated. Synthetic lifecycle tests and real return-to-render observations do not close those gaps. The existing performance report also explicitly leaves a broader UI ownership audit deferred; it does not establish leak-free UI teardown.

## HDMI audio acceptance

Physical HDMI tone attribution and audible playback remain unverified. Synthetic audio and lifecycle tests do not establish end-to-end hardware behavior. The checks below remain open until they are exercised on a supported module and a known-working HDMI audio source.

Use a quiet bounded HDMI sample. Do not disable the host audio server, alter global mixer/routing settings, or change unrelated target configuration to force a passing result.

- [ ] Correlate the chosen video node and unique capture PCM with the same physical supported USB `3188:1000` device, including when using `--device`. Do not substitute a webcam, microphone, default input or audio-server monitor source.
- [ ] Attribute a quiet known HDMI tone to that exact capture PCM: levels rise when the target tone is on and return to silence when it is off. Successful negotiation or silence-only capture is not a pass.
- [ ] Launch the exact wrapped Nix package and separately check the default Cargo/release runtime with its documented plugins. Confirm fresh-process mute at 25% and actual audible host output after unmuting; obtain the user's audibility confirmation if the tester cannot hear it.
- [ ] Exercise the speaker button and 0–100% slider: moving above zero unmutes, zero mutes, unmuting from zero restores 25%, and muting at a nonzero value preserves it. Check actual mouse/touch gestures, keyboard accessibility, and that a slider gesture lasting over three seconds keeps the overlay available without triggering fullscreen.
- [ ] Hold only the observed KVM capture PCM with an owned, bounded diagnostic, then unmute the viewer. Video must remain live with an audio-only warning beside the slider, no default-input fallback and no automatic busy retry loop. Release the diagnostic and confirm mute/unmute recovers audio without stopping the audio server or another user's recorder.
- [ ] With `background-throttle`, hide/minimize and return: audio stops during video suspension and resumes only once video is actually Playing. Visible focus loss alone must not mute. Check the default non-throttled build's documented behavior separately.
- [ ] Unplug/replug, including ALSA card renumbering, and confirm audio is re-associated with the module rather than a microphone. Retain mute/volume choice across reconnect and Stop/Start; Stop followed by replug or showing the window must remain stopped until Start.
- [ ] Exercise absent/unsupported audio, missing plugins and permission failures: video continues, the warning remains local to audio, and nonzero slider changes do not repeatedly reopen a failed source. A late capture PCM gets only bounded settling retries.
- [ ] Confirm mute, Stop, video error, unplug, close and enabled background pause release the viewer's audio stream and cancel pending settling work. Repeated retry/resume/error cycles must not accumulate owned graphs or bus watches; unrelated audio-server clients are not evidence of a viewer leak.
