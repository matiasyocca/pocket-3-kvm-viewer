mod capture;
mod device;
mod ui;

use gstreamer as gst;
use gtk4::prelude::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let initial_device = match capture::parse_args(&args) {
        Ok(device) => device,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    if let Err(err) = gst::init() {
        eprintln!("GStreamer initialization failed: {err}");
        std::process::exit(1);
    }
    if let Err(err) = gstgtk4::plugin_register_static() {
        eprintln!("GTK4 video sink registration failed: {err}");
        std::process::exit(1);
    }
    let app = gtk4::Application::builder()
        .application_id("io.github.pocket3.KvmViewer")
        .build();
    app.connect_activate(move |app| ui::build(app, initial_device.as_deref()));
    app.run_with_args(&["gpd-pocket3-kvm"]);
}
