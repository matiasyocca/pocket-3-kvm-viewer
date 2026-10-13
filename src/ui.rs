use std::{cell::RefCell, path::Path, rc::Rc};

use gst::prelude::*;
use gstreamer as gst;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::{capture, device};

struct Running {
    pipeline: gst::Pipeline,
    watch: gst::bus::BusWatchGuard,
}

fn stop(active: &Rc<RefCell<Option<Running>>>) {
    if let Some(running) = active.borrow_mut().take() {
        drop(running.watch);
        if let Err(err) = running.pipeline.set_state(gst::State::Null) {
            eprintln!("Could not stop capture: {err}");
        }
    }
}

fn populate(
    model: &gtk::StringList,
    dropdown: &gtk::DropDown,
    status: &gtk::Label,
    show_all: bool,
    preferred: Option<&str>,
) {
    model.splice(0, model.n_items(), &[]);
    let dev = Path::new("/dev");
    let nodes = if show_all {
        device::list_video_nodes(dev)
    } else {
        device::list_kvm_nodes(dev, Path::new("/sys/class/video4linux"))
    };
    match nodes {
        Ok(nodes) if nodes.is_empty() => status.set_text(if show_all {
            "No video nodes. Insert the KVM module and refresh."
        } else {
            "No KVM capture device (USB 3188:1000) found. Try Show all video devices."
        }),
        Ok(nodes) => {
            let selected = device::preferred_index(&nodes, preferred);
            for node in &nodes {
                let value = node.to_string_lossy();
                model.append(&value);
            }
            dropdown.set_selected(selected.unwrap_or(gtk::INVALID_LIST_POSITION));
            status.set_text(if selected.is_some() {
                "KVM capture device selected. Tap Start / Stop."
            } else {
                "Select a capture device, then tap Start / Stop."
            });
        }
        Err(err) => status.set_text(&format!("Cannot list /dev: {err}")),
    }
}

pub fn build(app: &gtk::Application, initial_device: Option<&str>) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Pocket 3 KVM")
        .default_width(960)
        .default_height(600)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let model = gtk::StringList::new(&[]);
    let dropdown = gtk::DropDown::builder().model(&model).build();
    let show_all = gtk::CheckButton::with_label("Show all video devices");
    show_all.set_active(initial_device.is_some());
    let refresh = gtk::Button::with_label("Refresh devices");
    let start = gtk::Button::with_label("Start / Stop");
    let fullscreen = gtk::Button::with_label("Full screen");
    let status = gtk::Label::new(Some("Select a capture device."));
    let picture = gtk::Picture::new();
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_can_shrink(true);
    let active: Rc<RefCell<Option<Running>>> = Rc::new(RefCell::new(None));
    controls.append(&dropdown);
    controls.append(&refresh);
    controls.append(&show_all);
    controls.append(&start);
    controls.append(&fullscreen);
    root.append(&controls);
    root.append(&picture);
    root.append(&status);
    window.set_child(Some(&root));
    populate(
        &model,
        &dropdown,
        &status,
        show_all.is_active(),
        initial_device,
    );

    {
        let (model, dropdown, status, show_all) = (
            model.clone(),
            dropdown.clone(),
            status.clone(),
            show_all.clone(),
        );
        refresh.connect_clicked(move |_| {
            let preferred = model.string(dropdown.selected());
            populate(
                &model,
                &dropdown,
                &status,
                show_all.is_active(),
                preferred.as_deref(),
            );
        });
    }
    {
        let (model, dropdown, status) = (model.clone(), dropdown.clone(), status.clone());
        show_all.connect_toggled(move |button| {
            let preferred = model.string(dropdown.selected());
            populate(
                &model,
                &dropdown,
                &status,
                button.is_active(),
                preferred.as_deref(),
            );
        });
    }
    {
        let (window, fullscreen_button) = (window.clone(), fullscreen.clone());
        fullscreen.connect_clicked(move |_| {
            if window.is_fullscreen() {
                window.unfullscreen();
                fullscreen_button.set_label("Full screen");
            } else {
                window.fullscreen();
                fullscreen_button.set_label("Exit full screen");
            }
        });
    }
    {
        let (active, model, dropdown, picture, status) = (
            active.clone(),
            model.clone(),
            dropdown.clone(),
            picture.clone(),
            status.clone(),
        );
        start.connect_clicked(move |_| {
            if active.borrow().is_some() {
                stop(&active);
                picture.set_paintable(None::<&gtk::gdk::Paintable>);
                status.set_text("Stopped.");
                return;
            }
            let Some(path) = model.string(dropdown.selected()) else {
                status.set_text("Select a video device first.");
                return;
            };
            if !capture::valid_device(&path) {
                status.set_text("Invalid video device path.");
                return;
            }
            let element = match gst::parse::launch(&capture::pipeline_description(&path)) {
                Ok(element) => element,
                Err(err) => {
                    status.set_text(&format!("Pipeline unavailable: {err}"));
                    return;
                }
            };
            let Ok(pipeline) = element.dynamic_cast::<gst::Pipeline>() else {
                status.set_text("Capture pipeline type error.");
                return;
            };
            let Some(sink) = pipeline.by_name("display") else {
                status.set_text("GTK4 video sink unavailable.");
                return;
            };
            let paintable = sink.property::<gtk::gdk::Paintable>("paintable");
            picture.set_paintable(Some(&paintable));
            let status_for_bus = status.clone();
            let watch = match pipeline.bus().expect("pipeline has a bus").add_watch_local(
                move |_, message| {
                    if let gst::MessageView::Error(err) = message.view() {
                        eprintln!("Capture error: {} ({:?})", err.error(), err.debug());
                        status_for_bus.set_text(&format!("Capture error: {}", err.error()));
                    }
                    glib::ControlFlow::Continue
                },
            ) {
                Ok(watch) => watch,
                Err(err) => {
                    status.set_text(&format!("Cannot watch capture: {err}"));
                    return;
                }
            };
            if let Err(err) = pipeline.set_state(gst::State::Playing) {
                drop(watch);
                status.set_text(&format!("Cannot start capture: {err}"));
                return;
            }
            *active.borrow_mut() = Some(Running { pipeline, watch });
            eprintln!("Started capture from {path}");
            status.set_text(&format!(
                "Capturing {path}; USB input forwarding is hardware-controlled."
            ));
        });
    }
    {
        let active = active.clone();
        window.connect_close_request(move |_| {
            stop(&active);
            glib::Propagation::Proceed
        });
    }
    window.present();
    if let Some(device) = initial_device {
        if let Some(index) =
            (0..model.n_items()).find(|&index| model.string(index).as_deref() == Some(device))
        {
            dropdown.set_selected(index);
            start.emit_clicked();
        } else {
            status.set_text(&format!(
                "Video node {device} is not present. Refresh devices."
            ));
        }
    }
}
