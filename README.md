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

Install the optional module. Connect the target's HDMI OUT to the module's HDMI IN (not the Pocket 3's own HDMI OUT). Launch the app, tap Refresh devices, select the correct `/dev/videoX` capture node, and tap Start / Stop. Use Full screen to expand the view and the same button to exit. Connect the target's USB port to the module's USB IN to use hardware keyboard/touchpad forwarding; use the touchscreen to operate the viewer if the physical keyboard/touchpad now control the target.

Video-node numbering is not stable. `v4l2-ctl --list-devices`, `v4l2-ctl -d /dev/videoX --all`, or `/sys/class/video4linux/video*/name` can identify the device. A camera or metadata-only node may appear in the list; try a different node if the view is wrong or the pipeline reports an error. The chosen `/dev/videoX` must be readable by the current user. If capture fails, check permissions and `GST_DEBUG=2 cargo run` before changing system permissions. Never run the GUI as root just to access a video node.

For an already identified node, `cargo run --release -- --device /dev/video2` opens the same window and starts capture immediately; substitute the actual device path. An invalid argument exits with a usage message. This is useful when the hardware routes the built-in keyboard to the target before you can select a device.

## Tests

`cargo fmt --check && cargo test && cargo clippy --all-targets -- -D warnings && cargo build --release`. After opening the GUI on real hardware, test live target BIOS/desktop video, fullscreen/exit, stop/start, HDMI reconnect, window close/reopen, and USB input switching and return. Measure latency/CPU and negotiated formats on the target machine rather than promising a particular frame rate. The pipeline favors fresh frames by using a two-buffer leaky queue and `sync=false`.

## Limitations

No audio, recording, automatic hot-plug launch, firmware controls, remote/IP KVM, or software input forwarding. Device selection is manual. Windows and macOS would require different capture backends and are not delivered by this release.

## Verified hardware (partial)

On the implementation host, DMI reported vendor GPD and product `G1621-02` with a Wayland/Hyprland session. `v4l2-ctl --list-devices` listed an `HDMI Capture: HDMI Capture` UVC device at `/dev/video2` plus a metadata node at `/dev/video3`. `/dev/video2` advertised YUYV 1920×1080 at 30 or 60 fps, and a 10-buffer GStreamer V4L2-to-fakesink capture completed without an error. The native viewer opened with `--device /dev/video2`; GStreamer negotiated YUY2 1920×1080 60/1 and its GTK4 sink created frames repeatedly over several seconds. This establishes that capture frames reached the sink, not their visible content, measured display latency, or USB forwarding/touchscreen behavior. Those manual checks remain open.
