# Architecture navigation

[User guide](../README.md) · [Development](DEVELOPMENT.md)

This is a map into the implementation, not an independent specification. Code, adjacent documentation, and tests define current behavior. Update the owning module when changing an invariant; update this map only when responsibilities or entry points change.

## Entry points and ownership

- [main.rs](../src/main.rs): argument handling, GStreamer initialization, static GTK sink registration, and GTK application activation.
- [device.rs](../src/device.rs): video-node enumeration and sysfs-based module identification. Discovery does not allocate a pipeline or own UI state.
- [capture.rs](../src/capture.rs): device-argument validation and the GStreamer graph description. The module documents the graph's latency/compatibility tradeoff; its tests cover accepted arguments and the graph contract.
- [audio.rs](../src/audio.rs): in-process mute/volume preferences, explicit capture-PCM association with the selected supported USB module, and independent playback graph ownership/retirement. It never selects a default input.
- [audio_controls.rs](../src/audio_controls.rs): window-scoped speaker/volume controls, audio-only warnings, and bounded missing-PCM settling. It owns the optional audio graph without making video depend on audio availability.
- [ui.rs](../src/ui.rs): GTK widgets and event-driven capture orchestration, including user intent, hot-plug, visibility, pipeline retirement, and focus-dependent inhibition. Its module documentation and lifecycle tests explain the distinction between manual Stop and a temporarily suspended pipeline.
- [inhibit.rs](../src/inhibit.rs): the logind D-Bus request and owned file descriptor that represents the sleep inhibitor. UI policy decides when to hold it.
- [telemetry.rs](../src/telemetry.rs): optional sink-buffer instrumentation, compiled only with `benchmark`. It does not measure compositor presentation or end-to-end latency.

The UI gates audio on both requested and actual video Playing state and retires audio on video teardown or background pause. Audio uses its own clock and the host's normal output; it does not delay or synchronize the video graph. Focus-dependent sleep inhibition is separate from audio policy: losing focus alone does not stop monitoring.

The module routes keyboard/touchpad input in hardware. There is no software input-forwarding subsystem or privileged helper in the application.

## Build and packaging boundaries

[Cargo.toml](../Cargo.toml) declares dependencies and feature gates; [Cargo.lock](../Cargo.lock) pins their resolution. [flake.nix](../flake.nix) defines the pinned development shell and wrapped runtime package. The [release workflow](../.github/workflows/release.yml) defines the conventional Linux release build and publication gates. Contributor commands and feature prerequisites are in [DEVELOPMENT.md](DEVELOPMENT.md).

## Rationale and evidence

Keep local design reasoning beside the relevant code rather than repeating implementation details here. The existing [performance report](PERFORMANCE.md) and its linked artifacts preserve measured optimization decisions, including accepted native-YUV/suspension work and rejected or deferred transport/GL approaches. Those are historical evidence, not a promise that every screened backend is implemented.

[Hardware validation notes](HARDWARE.md) distinguish previous observations from remaining physical checks. Neither this map nor a passing unit suite establishes unperformed hardware tests.

For a future consequential cross-module decision that cannot be explained locally, add a short decision record with context, alternatives, tradeoffs, and links to implementation/evidence. Do not create records for routine edits or duplicate the existing performance decisions.
