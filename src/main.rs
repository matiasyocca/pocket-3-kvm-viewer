mod capture;
mod device;
mod ui;

use gstreamer as gst;
use gtk4::prelude::*;

fn main() {
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
    app.connect_activate(ui::build);
    app.run();
}
