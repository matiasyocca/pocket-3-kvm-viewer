# Pocket 3 KVM viewer

A Linux-first Rust/GTK4 application for the optional GPD Pocket 3 KVM module's HDMI-IN capture. The module, not this application, redirects the built-in keyboard and touchpad when its USB cable is connected to a target. There is no keyboard injection or privileged helper.

## Build

Rust 1.92 or newer (required by `gst-plugin-gtk4` 0.15), Cargo, `pkg-config`, GTK4 headers, GStreamer core/base development headers, and GStreamer base/good runtime plugins are required. Build on either x86_64 or aarch64 Linux. The GTK4 paintable sink is compiled and registered from the Rust dependency; it is not necessary to install a separate system GTK4 GStreamer plugin.

On a conventional distribution, install the system dependencies first. Example package names (check your distribution's actual versions):

- Debian/Ubuntu: `libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-base gstreamer1.0-plugins-good pkg-config build-essential`
- Fedora: `gtk4-devel gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-good pkgconf-pkg-config gcc`
- Arch Linux: `gtk4 gstreamer gst-plugins-base gst-plugins-good pkgconf base-devel`

Then run:

```sh
pkg-config --modversion gtk4 gstreamer-1.0 gstreamer-video-1.0
cargo test
cargo build --release
./target/release/gpd-pocket3-kvm
```

For Nix with flakes enabled, use the pinned development shell (x86_64-linux and aarch64-linux):

```sh
nix develop
cargo test
cargo run --release
```

Alternatively run one command without entering a shell: `nix develop -c cargo run --release`. The Nix shell supplies the same native libraries that a regular distribution installs as packages. The Rust binary has runtime GTK4/GStreamer shared-library dependencies; it is not a universal static binary. Builds on macOS/Windows are not supported: the capture source here is Linux V4L2.

## Run

Install the optional module. Connect the target's HDMI OUT to the module's HDMI IN (not the Pocket 3's own HDMI OUT). Launch the app: it finds the observed module USB ID `3188:1000`, excludes the metadata-only node, and immediately starts capture if there is exactly one match. Video fills the window under a small overlay with Stop/Start and Full screen controls; there is no device dropdown, Refresh button, or persistent status footer. Double-click the video area with the primary button to enter or leave full screen; the overlay Full screen button remains available for touch. Start rescans devices after a stop or a failed initial detection, so tap it after hot-plugging the module. If the running pipeline reports a disconnection, tap Stop and then Start to reconnect. Errors and no-device messages appear over the video area only when needed. Connect the target's USB port to the module's USB IN to use hardware keyboard/touchpad forwarding; use the touchscreen to operate the controls if the physical keyboard/touchpad now control the target.

Video-node numbering is not stable. `v4l2-ctl --list-devices`, `v4l2-ctl -d /dev/videoX --all`, or `/sys/class/video4linux/video*/name` can identify the device when needed. The chosen `/dev/videoX` must be readable by the current user. If capture fails, check permissions and `GST_DEBUG=2 cargo run` before changing system permissions. Never run the GUI as root just to access a video node.

While capture is running and this window has focus, the viewer asks `systemd-logind` over the system D-Bus for an `idle:sleep` blocking inhibitor. This is independent of the window manager and desktop environment; `systemd-inhibit --list` should show `Pocket 3 KVM` while the window is focused. Focus loss, Stop, a pipeline error, window close, or process exit releases the inhibitor. If logind is unavailable or denies it, capture still works and the failure is printed to stderr. This only applies to power management honoring logind inhibitors; privileged forced suspend, lid handling configured to ignore inhibitors, or compositor-specific idle actions may still occur. Currently “source connected” means the UVC capture pipeline started successfully; the capture device does not expose a verified HDMI-signal-present indication, so an attached module without a working HDMI signal may also hold the lock until Stop or an error.

For a module revision with a different USB ID, or if multiple matching devices are attached, `cargo run --release -- --device /dev/video2` overrides discovery and starts the specified node immediately; substitute the actual device path. An invalid argument exits with a usage message.

## Tests

`cargo fmt --check && cargo test && cargo clippy --all-targets -- -D warnings && cargo build --release`. On a logind host with `systemd-inhibit` installed, run `cargo test focused_capture_holds_and_releases_a_logind_lock -- --ignored` to verify acquisition and release of a real blocking lock. With an attached module, verify discovery using `KVM_EXPECTED_DEVICE=/dev/video2 cargo test finds_attached_kvm_capture_node -- --ignored` (replace the path). After opening the GUI on real hardware, check `systemd-inhibit --list` while focused, unfocused, and after Stop; test live target BIOS/desktop video, fullscreen/exit, stop/start, HDMI reconnect, window close/reopen, and USB input switching and return. Measure latency/CPU and negotiated formats on the target machine rather than promising a particular frame rate. The pipeline favors fresh frames by using a two-buffer leaky queue and `sync=false`.

## Limitations

No audio, recording, automatic hot-plug launch, firmware controls, remote/IP KVM, or software input forwarding. Alternate capture nodes require the `--device` argument. The video retains its original aspect ratio; if the window has a different aspect ratio, empty margins may appear rather than cropping BIOS content. Windows and macOS would require different capture backends and are not delivered by this release.

## Verified hardware (partial)

On the implementation host, DMI reported vendor GPD and product `G1621-02` with a Wayland/Hyprland session. `v4l2-ctl --list-devices` listed an `HDMI Capture: HDMI Capture` UVC device at `/dev/video2` plus a metadata node at `/dev/video3`. Sysfs reports `3188:1000`, with V4L2 index 0 for `/dev/video2` and index 1 for `/dev/video3`. The hardware discovery test passed with `/dev/video2`. That node advertised YUYV 1920×1080 at 30 or 60 fps, and a 10-buffer GStreamer V4L2-to-fakesink capture completed without an error. The native viewer auto-opened `/dev/video2`; GStreamer negotiated YUY2 1920×1080 60/1, its GTK4 sink created frames repeatedly, and a scoped screenshot showed a target NixOS boot console rendered in the video pane with overlay controls and no footer/dropdown/checkbox. Display latency, USB forwarding, and touchscreen behavior remain unverified.
