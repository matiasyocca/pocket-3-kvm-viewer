//! Hardware-free diagnostic for compositor visibility hints.
use gtk4::{self as gtk, prelude::*};

fn build_application() -> gtk::Application {
    gtk::Application::builder()
        .application_id("io.github.pocket3.KvmViewer")
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build()
}

fn main() {
    let app = build_application();
    app.connect_activate(|app| {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("KVM visibility diagnostic")
            .default_width(960)
            .default_height(540)
            .build();
        window.set_child(Some(&gtk::Label::new(Some(
            "Compositor visibility diagnostic — no capture device is opened",
        ))));
        window.connect_realize(|window| {
            let surface = window.surface().unwrap();
            let toplevel = surface.downcast::<gtk::gdk::Toplevel>().unwrap();
            eprintln!("VIS initial {:?}", toplevel.state());
            toplevel.connect_state_notify(|top| eprintln!("VIS state {:?}", top.state()));
        });
        window.connect_map(|_| eprintln!("VIS mapped"));
        window.connect_unmap(|_| eprintln!("VIS unmapped"));
        window.connect_notify_local(Some("is-active"), |w, _| {
            eprintln!("VIS focused {}", w.is_active())
        });
        window.present();
    });
    app.run_with_args(&["visibility-diagnostic"]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a GTK display; never registers an application, presents a window, or opens a capture device"]
    fn diagnostic_cannot_cross_activate_production_application() {
        gtk::test_synced(|| {
            let app = build_application();
            assert!(!app.is_registered());
            assert_eq!(
                app.application_id().as_deref(),
                Some("io.github.pocket3.KvmViewer")
            );
            assert!(
                app.flags().contains(gtk::gio::ApplicationFlags::NON_UNIQUE),
                "the hardware-free diagnostic must not activate the production application"
            );
        });
    }
}
