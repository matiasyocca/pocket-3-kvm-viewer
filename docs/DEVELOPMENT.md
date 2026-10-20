# Development

[User guide](../README.md) · [Architecture navigation](ARCHITECTURE.md) · [Hardware validation](HARDWARE.md)

This guide covers contributor workflows. Dependency declarations and build configuration live in [Cargo.toml](../Cargo.toml), [Cargo.lock](../Cargo.lock), and [flake.nix](../flake.nix); release automation lives in the [workflow](../.github/workflows/release.yml). Update those definitions first when changing requirements.

## Build from source

Supported build targets are x86_64 and aarch64 Linux. The flake declares both, but only x86_64-linux has been build-tested. Capture uses Linux V4L2; macOS and Windows need different backends and are not supported.

### Conventional Linux distribution

Install Rust 1.92 or newer (required by `gst-plugin-gtk4` 0.15), Cargo, `pkg-config`, GTK4 4.10+ development headers, GStreamer core/base development headers, and GStreamer base/good and ALSA runtime plugins. Example packages; check your distribution's versions:

- Debian/Ubuntu: `libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-alsa pkg-config build-essential`
- Fedora: `gtk4-devel gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-good pkgconf-pkg-config gcc`
- Arch Linux: `gtk4 gstreamer gst-plugins-base gst-plugins-good pkgconf base-devel`

The GTK4 paintable sink is compiled and registered from the Rust dependency; do not install a separate system GTK4 GStreamer plugin for this application.

```sh
pkg-config --modversion gtk4 gstreamer-1.0 gstreamer-video-1.0
cargo test --locked
cargo build --locked --release
./target/release/pocket3-kvm-viewer
```

The executable uses GTK/GStreamer shared libraries; it is not a universal static binary.

### Pinned Nix development shell

With flakes enabled:

```sh
nix develop
pkg-config --modversion gtk4 gstreamer-1.0 gstreamer-video-1.0
cargo test --locked
cargo run --locked --release
```

Or run a single command with `nix develop -c cargo run --locked --release`. The shell supplies the native build libraries but does not enable Cargo features automatically. The installable Nix package has its own feature selection in `flake.nix`.

## Optional build features

Feature definitions are in `Cargo.toml`:

| Feature | Requirement | Purpose |
| --- | --- | --- |
| `native-yuv` | GTK 4.20+ | Enable the sink's native YUV texture support so compatible frames can bypass CPU RGB conversion without lowering the capture mode. |
| `background-throttle` | GTK 4.12+ | Pause/resume capture from GTK map/unmap and compositor suspension/minimization signals. |
| `benchmark` | Normal build dependencies | Add stderr frame-delivery and negotiated-caps diagnostics; not included in ordinary builds. |

For both rendering/visibility options on GTK 4.20+:

```sh
cargo run --locked --release --features native-yuv,background-throttle
```

Default Cargo builds retain the GTK 4.10-compatible path. The Nix package enables `native-yuv` and `background-throttle` with its pinned GTK, but not `benchmark`. The release workflow builds default features. Rendering and lifecycle rationale is documented beside the implementation; measured comparisons and rejected/deferred optimizations are in [PERFORMANCE.md](PERFORMANCE.md), not a second implementation specification.

## Automated checks

Run from the repository root in a provisioned environment (`nix develop` on NixOS):

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

Also exercise the optional code paths:

```sh
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo doc --locked --no-deps --document-private-items --all-features
python3 -B -m unittest discover -s scripts -p 'test_*.py' -v
```

All-features checks require GTK 4.20+. Include private items when generating Rust documentation because the implementation is a binary crate with internal modules.

Some Rust tests are ignored because they require a graphical session, logind, attached hardware, or explicit diagnostic setup. A passing default test run does not establish those checks. See [HARDWARE.md](HARDWARE.md) for opt-in commands and manual acceptance checks. See [PERFORMANCE.md](PERFORMANCE.md#method) for the benchmark harness, raw evidence, and measurement limits; sink-delivery FPS is not panel presentation rate or total power consumption.

## Publishing releases

The [release workflow](../.github/workflows/release.yml) is authoritative for publication gates and assets. It checks that the tag matches `Cargo.toml`, runs formatting/tests/Clippy, and builds a default-feature executable on Ubuntu 24.04 x86_64. It publishes a `.tar.gz` archive and SHA-256 checksum only after those steps succeed. It does not build an aarch64 archive.

Before tagging, ensure the workflow and intended code are committed and pushed, update the version declarations in `Cargo.toml` and `flake.nix` together, and refresh the local package's version entry in `Cargo.lock`. Verify the normal and optional-feature checks above. Then create and push the corresponding tag, replacing `vX.Y.Z` with the actual version:

```sh
git tag vX.Y.Z
git push origin vX.Y.Z
```

Inspect the hosted workflow run and release assets before calling a release successful. Download the published archive and matching checksum, verify it with `sha256sum -c`, and check the extracted executable in an appropriate graphical/runtime environment. A local build does not verify hosted publication or compatibility with every distribution.

Keep user-facing runtime requirements in the [installation guide](../README.md#linux-release-binary). Release archives retain the build host's shared-library/ABI requirements; they are not interchangeable with the wrapped Nix package.

## Audio development

`src/audio.rs` owns in-process preferences, safe USB/ALSA association and independent playback lifetime. `src/audio_controls.rs` owns the overlay controls, bounded enumeration settling and audio-only errors. `src/ui.rs` gates monitoring on the current video's requested and actual Playing state; it does not synchronize video to the audio clock. Preferences survive graph replacement in the current process, not application exit.

The `Opener` function pointer and `Playback::with_endpoints` permit hardware-free synthetic tests. Test audio endpoints must be `audiotestsrc`/`fakesink`, never a default microphone or actual speakers. GTK fixtures are ignored by the default test run and must be selected individually in a graphical session using their fully qualified names. Synthetic gain, lifecycle, retry and error-isolation coverage is not proof of audible HDMI playback; keep the [hardware acceptance checklist](HARDWARE.md#hdmi-audio-acceptance) separate.

Ubuntu/Debian runtimes need `gstreamer1.0-alsa` in addition to the existing GStreamer base/good plugins. This is a runtime plugin, not a new Rust crate or ALSA C development dependency. The existing Nix base/good plugin closure supplies ALSA; keep the runtime wrapper's GStreamer core plugin path as well. Check the required factories and exercise the exact wrapped package before claiming packaged audio support; a development-shell result alone is insufficient.

A busy ALSA capture PCM is an audio-only failure. Only a missing associated capture PCM receives bounded enumeration-settling retries; busy, plugin and permission failures must not trigger an automatic reopen loop or retries on nonzero gain changes. Mute then unmute is the explicit retry action. Do not fall back to a microphone, change system routing, disable the audio server, or change the target's HDMI configuration as part of application recovery.
