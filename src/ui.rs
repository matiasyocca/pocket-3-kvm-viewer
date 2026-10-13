use std::{cell::RefCell, path::Path, rc::Rc, sync::Once, time::Duration};

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

fn is_double_click(n_press: i32) -> bool {
    n_press == 2
}

fn reveals_controls(
    event: gtk::gdk::EventType,
    position: Option<(f64, f64)>,
    last_pointer: &mut Option<(f64, f64)>,
) -> bool {
    match event {
        gtk::gdk::EventType::TouchBegin => true,
        gtk::gdk::EventType::MotionNotify => {
            let Some(position) = position else {
                return false;
            };
            if *last_pointer == Some(position) {
                return false;
            }
            *last_pointer = Some(position);
            true
        }
        _ => false,
    }
}

fn reset_controls_timer(controls: &gtk::Box, pending: &Rc<RefCell<Option<glib::SourceId>>>) {
    controls.set_visible(true);
    if let Some(id) = pending.borrow_mut().take() {
        id.remove();
    }
    let weak_controls = controls.downgrade();
    let pending_for_timeout = pending.clone();
    let id = glib::timeout_add_local_once(Duration::from_secs(3), move || {
        pending_for_timeout.borrow_mut().take();
        if let Some(controls) = weak_controls.upgrade() {
            controls.set_visible(false);
        }
    });
    *pending.borrow_mut() = Some(id);
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

    let pending_hide = Rc::new(RefCell::new(None));
    reset_controls_timer(&controls, &pending_hide);
    let activity = gtk::EventControllerLegacy::new();
    activity.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let controls = controls.clone();
        let pending_hide = pending_hide.clone();
        let last_pointer = RefCell::new(None);
        activity.connect_event(move |_, event| {
            if reveals_controls(
                event.event_type(),
                event.position(),
                &mut last_pointer.borrow_mut(),
            ) {
                reset_controls_timer(&controls, &pending_hide);
            }
            glib::Propagation::Proceed
        });
    }
    overlay.add_controller(activity);

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
    let double_click = gtk::GestureClick::new();
    double_click.set_button(1); // Primary mouse/touchpad button only.
    {
        let fullscreen = fullscreen.clone();
        double_click.connect_pressed(move |_, n_press, _, _| {
            if is_double_click(n_press) {
                fullscreen.emit_clicked();
            }
        });
    }
    picture.add_controller(double_click);
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
    use super::{is_double_click, reveals_controls, should_inhibit};

    #[test]
    fn pointer_motion_or_touch_begin_reveals_controls() {
        use gtk4::gdk::EventType;
        let mut last_pointer = None;
        assert!(reveals_controls(
            EventType::MotionNotify,
            Some((1.0, 1.0)),
            &mut last_pointer,
        ));
        assert!(reveals_controls(
            EventType::TouchBegin,
            None,
            &mut last_pointer,
        ));
        assert!(!reveals_controls(
            EventType::TouchUpdate,
            None,
            &mut last_pointer,
        ));
        assert!(!reveals_controls(
            EventType::ButtonPress,
            None,
            &mut last_pointer,
        ));
    }

    #[test]
    fn stationary_pointer_does_not_reveal_controls_after_hide() {
        use gtk4::gdk::EventType;
        let mut last_pointer = None;
        assert!(reveals_controls(
            EventType::MotionNotify,
            Some((42.0, 24.0)),
            &mut last_pointer,
        ));
        // The timer hides the controls. GTK may repick the widget under a stationary cursor.
        assert!(!reveals_controls(
            EventType::MotionNotify,
            Some((42.0, 24.0)),
            &mut last_pointer,
        ));
        assert!(reveals_controls(
            EventType::MotionNotify,
            Some((44.0, 24.0)),
            &mut last_pointer,
        ));
        assert!(reveals_controls(
            EventType::TouchBegin,
            None,
            &mut last_pointer,
        ));
    }

    #[test]
    fn toggles_only_on_second_click() {
        assert!(!is_double_click(1));
        assert!(is_double_click(2));
        assert!(!is_double_click(3));
    }

    #[test]
    fn inhibits_only_while_capture_is_active_and_window_is_focused() {
        assert!(!should_inhibit(false, false));
        assert!(!should_inhibit(false, true));
        assert!(!should_inhibit(true, false));
        assert!(should_inhibit(true, true));
    }
}
