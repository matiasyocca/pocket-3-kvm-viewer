use std::{cell::RefCell, path::Path, rc::Rc, sync::Once};

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

type ActiveCapture = Rc<RefCell<Option<Running>>>;
static INSTALL_STYLE: Once = Once::new();

fn should_inhibit(capture_active: bool, window_focused: bool) -> bool {
    capture_active && window_focused
}

struct InhibitController {
    window: glib::WeakRef<gtk::ApplicationWindow>,
    state: RefCell<(bool, Option<crate::inhibit::SleepInhibitor>)>,
}

impl InhibitController {
    fn new(window: &gtk::ApplicationWindow) -> Self {
        Self {
            window: window.downgrade(),
            state: RefCell::new((false, None)),
        }
    }

    fn set_capture_active(&self, active: bool) {
        self.state.borrow_mut().0 = active;
        self.update();
    }

    fn update(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let mut state = self.state.borrow_mut();
        if should_inhibit(state.0, window.is_active()) {
            if state.1.is_none() {
                match crate::inhibit::SleepInhibitor::acquire() {
                    Ok(lock) => {
                        state.1 = Some(lock);
                        eprintln!("Sleep/idle inhibition active");
                    }
                    Err(err) => eprintln!("Could not inhibit sleep/idle: {err}"),
                }
            }
        } else if state.1.take().is_some() {
            eprintln!("Sleep/idle inhibition released");
        }
    }
}

fn notice(label: &gtk::Label, message: &str) {
    label.set_text(message);
    label.set_visible(true);
}

fn stop(
    active: &ActiveCapture,
    picture: &gtk::Picture,
    button: &gtk::Button,
    inhibitor: &InhibitController,
) {
    inhibitor.set_capture_active(false);
    if let Some(running) = active.borrow_mut().take() {
        drop(running.watch);
        if let Err(err) = running.pipeline.set_state(gst::State::Null) {
            eprintln!("Could not stop capture: {err}");
        }
    }
    picture.set_paintable(None::<&gtk::gdk::Paintable>);
    button.set_label("Start");
}

fn start_capture(
    path: &Path,
    active: &ActiveCapture,
    picture: &gtk::Picture,
    message: &gtk::Label,
    button: &gtk::Button,
    inhibitor: &Rc<InhibitController>,
) -> Result<(), String> {
    let path = path.to_string_lossy();
    if !capture::valid_device(&path) {
        return Err("Invalid video device path.".to_owned());
    }
    let element = gst::parse::launch(&capture::pipeline_description(&path))
        .map_err(|err| format!("Pipeline unavailable: {err}"))?;
    let pipeline = element
        .dynamic_cast::<gst::Pipeline>()
        .map_err(|_| "Capture pipeline type error.".to_owned())?;
    let sink = pipeline
        .by_name("display")
        .ok_or_else(|| "GTK4 video sink unavailable.".to_owned())?;
    let paintable = sink.property::<gtk::gdk::Paintable>("paintable");
    picture.set_paintable(Some(&paintable));
    let error_label = message.clone();
    let error_inhibitor = inhibitor.clone();
    let watch = pipeline
        .bus()
        .expect("pipeline has a bus")
        .add_watch_local(move |_, event| {
            if let gst::MessageView::Error(err) = event.view() {
                eprintln!("Capture error: {} ({:?})", err.error(), err.debug());
                error_inhibitor.set_capture_active(false);
                notice(&error_label, &format!("Capture error: {}", err.error()));
            }
            glib::ControlFlow::Continue
        })
        .map_err(|err| format!("Cannot watch capture: {err}"))?;
    if let Err(err) = pipeline.set_state(gst::State::Playing) {
        drop(watch);
        picture.set_paintable(None::<&gtk::gdk::Paintable>);
        return Err(format!("Cannot start capture: {err}"));
    }
    *active.borrow_mut() = Some(Running { pipeline, watch });
    inhibitor.set_capture_active(true);
    message.set_visible(false);
    button.set_label("Stop");
    eprintln!("Started capture from {path}");
    Ok(())
}

pub fn build(app: &gtk::Application, initial_device: Option<&str>) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Pocket 3 KVM")
        .default_width(960)
        .default_height(540)
        .build();
    let overlay = gtk::Overlay::new();
    overlay.add_css_class("kvm-view");
    INSTALL_STYLE.call_once(|| {
        let css = gtk::CssProvider::new();
        css.load_from_data(".kvm-view { background-color: #000; }");
        gtk::style_context_add_provider_for_display(
            &gtk::prelude::WidgetExt::display(&window),
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
    let picture = gtk::Picture::new();
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Contain);
    overlay.set_child(Some(&picture));

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.add_css_class("osd");
    controls.set_halign(gtk::Align::End);
    controls.set_valign(gtk::Align::Start);
    controls.set_margin_top(8);
    controls.set_margin_end(8);
    let start = gtk::Button::with_label("Start");
    let fullscreen = gtk::Button::with_label("Full screen");
    controls.append(&start);
    controls.append(&fullscreen);
    overlay.add_overlay(&controls);

    let message = gtk::Label::new(None);
    message.add_css_class("osd");
    message.set_wrap(true);
    message.set_halign(gtk::Align::Center);
    message.set_valign(gtk::Align::Center);
    message.set_visible(false);
    overlay.add_overlay(&message);
    window.set_child(Some(&overlay));

    let active: ActiveCapture = Rc::new(RefCell::new(None));
    let inhibitor = Rc::new(InhibitController::new(&window));
    let explicit = initial_device.map(str::to_owned);

    {
        let inhibitor = inhibitor.clone();
        window.connect_notify_local(Some("is-active"), move |_, _| inhibitor.update());
    }

    {
        let (active, picture, message, button, inhibitor) = (
            active.clone(),
            picture.clone(),
            message.clone(),
            start.clone(),
            inhibitor.clone(),
        );
        start.connect_clicked(move |_| {
            if active.borrow().is_some() {
                stop(&active, &picture, &button, &inhibitor);
                notice(&message, "Stopped. Tap Start to resume.");
            } else {
                match device::resolve_capture_node(
                    Path::new("/dev"),
                    Path::new("/sys/class/video4linux"),
                    explicit.as_deref(),
                ) {
                    Ok(path) => {
                        if let Err(err) =
                            start_capture(&path, &active, &picture, &message, &button, &inhibitor)
                        {
                            notice(&message, &err);
                        }
                    }
                    Err(err) => notice(&message, &err),
                }
            }
        });
    }
    {
        let (window, button) = (window.downgrade(), fullscreen.clone());
        fullscreen.connect_clicked(move |_| {
            let Some(window) = window.upgrade() else {
                return;
            };
            if window.is_fullscreen() {
                window.unfullscreen();
                button.set_label("Full screen");
            } else {
                window.fullscreen();
                button.set_label("Exit full screen");
            }
        });
    }
    {
        let (active, picture, button, inhibitor) = (
            active.clone(),
            picture.clone(),
            start.clone(),
            inhibitor.clone(),
        );
        window.connect_close_request(move |_| {
            stop(&active, &picture, &button, &inhibitor);
            glib::Propagation::Proceed
        });
    }
    window.present();
    start.emit_clicked();
}

#[cfg(test)]
mod tests {
    use super::should_inhibit;

    #[test]
    fn inhibits_only_while_capture_is_active_and_window_is_focused() {
        assert!(!should_inhibit(false, false));
        assert!(!should_inhibit(false, true));
        assert!(!should_inhibit(true, false));
        assert!(should_inhibit(true, true));
    }
}
