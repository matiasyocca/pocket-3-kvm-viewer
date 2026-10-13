use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Once,
    time::Duration,
};

use gst::prelude::*;
use gstreamer as gst;
use gtk::prelude::*;
use gtk::{gio, glib};
use gtk4 as gtk;

use crate::{capture, device};

struct Running {
    path: PathBuf,
    pipeline: gst::Pipeline,
    requested_state: Cell<gst::State>,
    watch: gst::bus::BusWatchGuard,
}

type ActiveCapture = Rc<RefCell<Option<Running>>>;
static INSTALL_STYLE: Once = Once::new();

#[derive(Clone, Copy, PartialEq, Eq)]
enum CapturePreference {
    Automatic,
    Paused,
}

#[cfg(feature = "background-throttle")]
fn capture_target_state(
    preference: CapturePreference,
    mapped: bool,
    state: gtk::gdk::ToplevelState,
) -> Option<gst::State> {
    if preference == CapturePreference::Paused {
        return None;
    }
    let hidden = !mapped
        || state
            .intersects(gtk::gdk::ToplevelState::SUSPENDED | gtk::gdk::ToplevelState::MINIMIZED);
    Some(if hidden {
        gst::State::Paused
    } else {
        gst::State::Playing
    })
}

fn should_autostart(preference: CapturePreference, active: bool) -> bool {
    preference == CapturePreference::Automatic && !active
}

fn is_video_node(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("video"))
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn removed_running_node(event: gtk::gio::FileMonitorEvent, changed: &Path, running: &Path) -> bool {
    matches!(
        event,
        gtk::gio::FileMonitorEvent::Deleted | gtk::gio::FileMonitorEvent::MovedOut
    ) && changed == running
}

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

fn cursor_name_for_controls(visible: bool) -> Option<&'static str> {
    if visible {
        None
    } else {
        Some("none")
    }
}

fn reset_controls_timer(
    controls: &gtk::Box,
    overlay: &gtk::Overlay,
    pending: &Rc<RefCell<Option<glib::SourceId>>>,
) {
    controls.set_visible(true);
    overlay.set_cursor_from_name(cursor_name_for_controls(true));
    if let Some(id) = pending.borrow_mut().take() {
        id.remove();
    }
    let weak_controls = controls.downgrade();
    let weak_overlay = overlay.downgrade();
    let pending_for_timeout = pending.clone();
    let id = glib::timeout_add_local_once(Duration::from_secs(3), move || {
        pending_for_timeout.borrow_mut().take();
        if let (Some(controls), Some(overlay)) = (weak_controls.upgrade(), weak_overlay.upgrade()) {
            controls.set_visible(false);
            overlay.set_cursor_from_name(cursor_name_for_controls(false));
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
    retire_capture(active, picture, inhibitor);
    button.set_label("Start");
}

fn retire_capture(active: &ActiveCapture, picture: &gtk::Picture, inhibitor: &InhibitController) {
    inhibitor.set_capture_active(false);
    if let Some(running) = active.borrow_mut().take() {
        drop(running.watch);
        if let Err(err) = running.pipeline.set_state(gst::State::Null) {
            eprintln!("Could not stop capture: {err}");
        }
    }
    picture.set_paintable(None::<&gtk::gdk::Paintable>);
}

// Track the requested state as well as actual state: async transitions must not
// be reissued on duplicate visibility notifications.
fn request_pipeline_state(
    running: &Running,
    target: gst::State,
    inhibitor: &InhibitController,
) -> Result<bool, String> {
    if running.requested_state.get() == target {
        return Ok(false);
    }
    running.requested_state.set(target);
    if target != gst::State::Playing {
        inhibitor.set_capture_active(false);
    }
    running
        .pipeline
        .set_state(target)
        .map_err(|err| format!("Cannot change capture state: {err}"))?;
    inhibitor.set_capture_active(
        target == gst::State::Playing && running.pipeline.current_state() == gst::State::Playing,
    );
    Ok(true)
}

#[cfg(feature = "background-throttle")]
fn toplevel_state(window: &gtk::ApplicationWindow) -> gtk::gdk::ToplevelState {
    window
        .surface()
        .and_then(|surface| surface.dynamic_cast::<gtk::gdk::Toplevel>().ok())
        .map_or(gtk::gdk::ToplevelState::empty(), |toplevel| {
            toplevel.state()
        })
}

#[cfg(feature = "background-throttle")]
fn capture_state_for_window(window: &gtk::ApplicationWindow) -> gst::State {
    capture_target_state(
        CapturePreference::Automatic,
        window.is_mapped(),
        toplevel_state(window),
    )
    .unwrap()
}

#[cfg(feature = "background-throttle")]
fn sync_capture_visibility(
    active: &ActiveCapture,
    preference: CapturePreference,
    mapped: bool,
    state: gtk::gdk::ToplevelState,
    widgets: (
        &gtk::Picture,
        &gtk::Label,
        Option<&gtk::Button>,
        &Rc<InhibitController>,
    ),
) -> Result<bool, String> {
    let Some(target) = capture_target_state(preference, mapped, state) else {
        return Ok(false);
    };
    let (picture, message, button, inhibitor) = widgets;
    let result = active.borrow().as_ref().map_or(Ok(false), |running| {
        request_pipeline_state(running, target, inhibitor)
    });
    if let Err(err) = &result {
        retire_capture(active, picture, inhibitor);
        if let Some(button) = button {
            button.set_label("Start");
        }
        notice(message, err);
    }
    result
}

#[cfg(feature = "background-throttle")]
fn take_initial_start(
    mapped: bool,
    preference: CapturePreference,
    allocated: bool,
    started: &Cell<bool>,
) -> bool {
    mapped && !started.replace(true) && should_autostart(preference, allocated)
}

// Signals only: compositor suspension/minimization and GTK map/unmap. Focus and
// pointer activity remain exclusively input/UI and inhibition concerns.
#[cfg(feature = "background-throttle")]
fn watch_capture_visibility(
    window: &gtk::ApplicationWindow,
    active: &ActiveCapture,
    preference: &Rc<Cell<CapturePreference>>,
    widgets: (
        &gtk::Picture,
        &gtk::Label,
        &gtk::Button,
        &Rc<InhibitController>,
    ),
) {
    let (picture, message, button, inhibitor) = widgets;
    let update = {
        let (active, preference, picture, message, button, inhibitor) = (
            Rc::downgrade(active),
            preference.clone(),
            picture.clone(),
            message.clone(),
            button.downgrade(),
            inhibitor.clone(),
        );
        Rc::new(move |window: &gtk::ApplicationWindow, mapped: bool| {
            if let Some(active) = active.upgrade() {
                if let Err(err) = sync_capture_visibility(
                    &active,
                    preference.get(),
                    mapped,
                    toplevel_state(window),
                    (&picture, &message, button.upgrade().as_ref(), &inhibitor),
                ) {
                    eprintln!("Capture visibility transition failed: {err}");
                }
            }
        })
    };
    // Surface state belongs to a realization, not to the window forever.
    let watched = Rc::new(RefCell::new(
        None::<(gtk::gdk::Toplevel, glib::SignalHandlerId)>,
    ));
    {
        let (watched, update) = (watched.clone(), update.clone());
        window.connect_realize(move |window| {
            if let Some((toplevel, handler)) = watched.borrow_mut().take() {
                toplevel.disconnect(handler);
            }
            if let Some(toplevel) = window
                .surface()
                .and_then(|surface| surface.dynamic_cast::<gtk::gdk::Toplevel>().ok())
            {
                let weak_window = window.downgrade();
                let update = update.clone();
                let handler = toplevel.connect_state_notify(move |_| {
                    if let Some(window) = weak_window.upgrade() {
                        update(&window, window.is_mapped());
                    }
                });
                *watched.borrow_mut() = Some((toplevel, handler));
            }
            update(window, window.is_mapped());
        });
    }
    {
        let update = update.clone();
        window.connect_unrealize(move |window| {
            if let Some((toplevel, handler)) = watched.borrow_mut().take() {
                toplevel.disconnect(handler);
            }
            update(window, false);
        });
    }
    {
        let (update, preference, button, active) = (
            update.clone(),
            preference.clone(),
            button.downgrade(),
            Rc::downgrade(active),
        );
        let started = Cell::new(false);
        window.connect_map(move |window| {
            update(window, true);
            let allocated = active
                .upgrade()
                .is_some_and(|active| active.borrow().is_some());
            if take_initial_start(true, preference.get(), allocated, &started) {
                if let Some(button) = button.upgrade() {
                    button.emit_clicked();
                }
            }
        });
    }
    window.connect_unmap(move |window| update(window, false));
}

fn start_capture(
    path: &Path,
    active: &ActiveCapture,
    picture: &gtk::Picture,
    message: &gtk::Label,
    button: &gtk::Button,
    inhibitor: &Rc<InhibitController>,
) -> Result<(), String> {
    let capture_path = path;
    let path = path.to_string_lossy();
    if !capture::valid_device(&path) {
        return Err("Invalid video device path.".to_owned());
    }
    let element = gst::parse::launch(&capture::pipeline_description(&path))
        .map_err(|err| format!("Pipeline unavailable: {err}"))?;
    let pipeline = element
        .dynamic_cast::<gst::Pipeline>()
        .map_err(|_| "Capture pipeline type error.".to_owned())?;
    #[cfg(feature = "benchmark")]
    crate::telemetry::install(&pipeline);
    let sink = pipeline
        .by_name("display")
        .ok_or_else(|| "GTK4 video sink unavailable.".to_owned())?;
    let paintable = sink.property::<gtk::gdk::Paintable>("paintable");
    picture.set_paintable(Some(&paintable));
    start_pipeline(
        capture_path,
        pipeline,
        active,
        (picture, message, button, inhibitor),
        {
            #[cfg(feature = "background-throttle")]
            {
                inhibitor
                    .window
                    .upgrade()
                    .map_or(gst::State::Paused, |window| {
                        capture_state_for_window(&window)
                    })
            }
            #[cfg(not(feature = "background-throttle"))]
            {
                gst::State::Playing
            }
        },
    )
}

// Keep pipeline lifecycle independent of the hardware source and GTK video sink.
fn start_pipeline(
    path: &Path,
    pipeline: gst::Pipeline,
    active: &ActiveCapture,
    widgets: (
        &gtk::Picture,
        &gtk::Label,
        &gtk::Button,
        &Rc<InhibitController>,
    ),
    target: gst::State,
) -> Result<(), String> {
    let (picture, message, button, inhibitor) = widgets;
    let error_label = message.clone();
    let error_inhibitor = inhibitor.clone();
    let error_active = Rc::downgrade(active);
    let error_picture = picture.clone();
    // Start's signal owns ActiveCapture; the bus/idle must not own Start.
    let error_button = button.downgrade();
    let watch = pipeline
        .bus()
        .expect("pipeline has a bus")
        .add_watch_local(move |bus, event| {
            if let gst::MessageView::StateChanged(_) = event.view() {
                if let Some(active) = error_active.upgrade() {
                    let current = active.borrow();
                    if let Some(running) = current.as_ref().filter(|running| {
                        running.pipeline.bus().as_ref() == Some(bus)
                            && event.src() == Some(running.pipeline.upcast_ref())
                    }) {
                        error_inhibitor.set_capture_active(
                            running.pipeline.current_state() == gst::State::Playing
                                && running.requested_state.get() == gst::State::Playing,
                        );
                    }
                }
            }
            if let gst::MessageView::Error(err) = event.view() {
                let Some(active) = error_active.upgrade() else {
                    return glib::ControlFlow::Continue;
                };
                if active
                    .borrow()
                    .as_ref()
                    .is_none_or(|running| running.pipeline.bus().as_ref() != Some(bus))
                {
                    return glib::ControlFlow::Continue;
                }
                eprintln!("Capture error: {} ({:?})", err.error(), err.debug());
                notice(&error_label, &format!("Capture error: {}", err.error()));
                let (active, picture, button, inhibitor, bus) = (
                    error_active.clone(),
                    error_picture.clone(),
                    error_button.clone(),
                    error_inhibitor.clone(),
                    bus.clone(),
                );
                // Removing this watch during its own dispatch would drop the callback.
                glib::idle_add_local_once(move || {
                    if let Some(active) = active.upgrade() {
                        let current = active
                            .borrow()
                            .as_ref()
                            .is_some_and(|running| running.pipeline.bus().as_ref() == Some(&bus));
                        // Start/Stop may have replaced the capture before this idle ran.
                        if current {
                            retire_capture(&active, &picture, &inhibitor);
                            if let Some(button) = button.upgrade() {
                                button.set_label("Start");
                            }
                        }
                    }
                });
            }
            glib::ControlFlow::Continue
        })
        .map_err(|err| format!("Cannot watch capture: {err}"))?;
    *active.borrow_mut() = Some(Running {
        path: path.to_path_buf(),
        pipeline: pipeline.clone(),
        requested_state: Cell::new(gst::State::Null),
        watch,
    });
    let result = request_pipeline_state(active.borrow().as_ref().unwrap(), target, inhibitor);
    if let Err(err) = result {
        stop(active, picture, button, inhibitor);
        return Err(format!("Cannot start capture: {err}"));
    }
    message.set_visible(false);
    button.set_label("Stop");
    eprintln!("Started capture from {}", path.display());
    Ok(())
}

fn try_start_capture(
    explicit: Option<&str>,
    active: &ActiveCapture,
    picture: &gtk::Picture,
    message: &gtk::Label,
    button: &gtk::Button,
    inhibitor: &Rc<InhibitController>,
) {
    let result = device::resolve_capture_node(
        Path::new("/dev"),
        Path::new("/sys/class/video4linux"),
        explicit,
    )
    .and_then(|path| start_capture(&path, active, picture, message, button, inhibitor));
    if let Err(err) = result {
        notice(message, &err);
    }
}

fn watch_video_nodes(
    window: &gtk::ApplicationWindow,
    active: &ActiveCapture,
    preference: &Rc<Cell<CapturePreference>>,
    widgets: (
        &gtk::Picture,
        &gtk::Label,
        &gtk::Button,
        &Rc<InhibitController>,
    ),
    explicit: Option<String>,
) -> Result<(), glib::Error> {
    let (picture, message, button, inhibitor) = widgets;
    let monitor = gio::File::for_path("/dev")
        .monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)?;
    let pending = Rc::new(RefCell::new(None::<glib::SourceId>));
    {
        let (window, active, preference, picture, message, button, inhibitor, pending) = (
            window.downgrade(),
            active.clone(),
            preference.clone(),
            picture.clone(),
            message.clone(),
            button.clone(),
            inhibitor.clone(),
            pending.clone(),
        );
        monitor.connect_changed(move |_, file, _, event| {
            let Some(path) = file.path() else {
                return;
            };
            if !is_video_node(&path) {
                return;
            }
            let running_removed = {
                let current = active.borrow();
                current
                    .as_ref()
                    .is_some_and(|running| removed_running_node(event, &path, &running.path))
            };
            if running_removed {
                stop(&active, &picture, &button, &inhibitor);
                notice(&message, "KVM disconnected. Waiting for the module...");
            }
            if !matches!(
                event,
                gio::FileMonitorEvent::Created
                    | gio::FileMonitorEvent::MovedIn
                    | gio::FileMonitorEvent::Deleted
                    | gio::FileMonitorEvent::MovedOut
            ) {
                return;
            }
            if let Some(id) = pending.borrow_mut().take() {
                id.remove();
            }
            let (window, active, preference, picture, message, button, inhibitor) = (
                window.clone(),
                active.clone(),
                preference.clone(),
                picture.clone(),
                message.clone(),
                button.clone(),
                inhibitor.clone(),
            );
            let pending_for_timeout = pending.clone();
            let explicit = explicit.clone();
            let id = glib::timeout_add_local_once(Duration::from_millis(500), move || {
                pending_for_timeout.borrow_mut().take();
                let already_running = active.borrow().is_some();
                if window.upgrade().is_some() && should_autostart(preference.get(), already_running)
                {
                    try_start_capture(
                        explicit.as_deref(),
                        &active,
                        &picture,
                        &message,
                        &button,
                        &inhibitor,
                    );
                }
            });
            *pending.borrow_mut() = Some(id);
        });
    }
    window.connect_close_request(move |_| {
        monitor.cancel();
        if let Some(id) = pending.borrow_mut().take() {
            id.remove();
        }
        glib::Propagation::Proceed
    });
    Ok(())
}

pub fn build(app: &gtk::Application, initial_device: Option<&str>) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Pocket 3 KVM Viewer")
        .default_width(960)
        .default_height(540)
        .build();
    let overlay = gtk::Overlay::new();
    overlay.add_css_class("kvm-view");
    INSTALL_STYLE.call_once(|| {
        let css = gtk::CssProvider::new();
        #[cfg(any(feature = "native-yuv", feature = "background-throttle"))]
        css.load_from_string(".kvm-view { background-color: #000; }");
        #[cfg(not(any(feature = "native-yuv", feature = "background-throttle")))]
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
    reset_controls_timer(&controls, &overlay, &pending_hide);
    let activity = gtk::EventControllerLegacy::new();
    activity.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let controls = controls.clone();
        let weak_overlay = overlay.downgrade();
        let pending_hide = pending_hide.clone();
        let last_pointer = RefCell::new(None);
        activity.connect_event(move |_, event| {
            if reveals_controls(
                event.event_type(),
                event.position(),
                &mut last_pointer.borrow_mut(),
            ) {
                if let Some(overlay) = weak_overlay.upgrade() {
                    reset_controls_timer(&controls, &overlay, &pending_hide);
                }
            }
            glib::Propagation::Proceed
        });
    }
    overlay.add_controller(activity);

    let active: ActiveCapture = Rc::new(RefCell::new(None));
    let preference = Rc::new(Cell::new(CapturePreference::Automatic));
    let inhibitor = Rc::new(InhibitController::new(&window));
    let explicit = initial_device.map(str::to_owned);
    let monitor_error = watch_video_nodes(
        &window,
        &active,
        &preference,
        (&picture, &message, &start, &inhibitor),
        explicit.clone(),
    )
    .err();
    if let Some(err) = &monitor_error {
        eprintln!("Automatic device monitoring unavailable: {err}");
    }

    {
        let inhibitor = inhibitor.clone();
        window.connect_notify_local(Some("is-active"), move |_, _| inhibitor.update());
    }

    {
        let (active, picture, message, button, inhibitor, preference) = (
            active.clone(),
            picture.clone(),
            message.clone(),
            start.clone(),
            inhibitor.clone(),
            preference.clone(),
        );
        let explicit = explicit.clone();
        start.connect_clicked(move |_| {
            if active.borrow().is_some() {
                preference.set(CapturePreference::Paused);
                stop(&active, &picture, &button, &inhibitor);
                notice(&message, "Paused. Tap Start to resume automatic capture.");
            } else {
                preference.set(CapturePreference::Automatic);
                try_start_capture(
                    explicit.as_deref(),
                    &active,
                    &picture,
                    &message,
                    &button,
                    &inhibitor,
                );
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
    #[cfg(feature = "background-throttle")]
    watch_capture_visibility(
        &window,
        &active,
        &preference,
        (&picture, &message, &start, &inhibitor),
    );
    window.present();
    #[cfg(not(feature = "background-throttle"))]
    start.emit_clicked();
    if monitor_error.is_some() && active.borrow().is_none() {
        notice(
            &message,
            "Device monitoring unavailable. Tap Start after connecting the module.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PipelineUi {
        active: ActiveCapture,
        picture: gtk::Picture,
        message: gtk::Label,
        button: gtk::Button,
        inhibitor: Rc<InhibitController>,
        _window: gtk::ApplicationWindow,
    }

    impl PipelineUi {
        fn new() -> Self {
            gst::init().unwrap();
            let window = gtk::ApplicationWindow::builder().build();
            Self {
                active: Rc::new(RefCell::new(None)),
                picture: gtk::Picture::new(),
                message: gtk::Label::new(None),
                button: gtk::Button::with_label("Start"),
                inhibitor: Rc::new(InhibitController::new(&window)),
                _window: window,
            }
        }

        fn start(&self, pipeline: &gst::Pipeline) -> Result<(), String> {
            let paintable = gtk::gdk::MemoryTexture::new(
                1,
                1,
                gtk::gdk::MemoryFormat::R8g8b8a8,
                &glib::Bytes::from_static(&[0, 0, 0, 255]),
                4,
            );
            self.picture.set_paintable(Some(&paintable));
            start_pipeline(
                Path::new("/etc/hosts"),
                pipeline.clone(),
                &self.active,
                (&self.picture, &self.message, &self.button, &self.inhibitor),
                gst::State::Playing,
            )
        }
    }

    impl Drop for PipelineUi {
        fn drop(&mut self) {
            stop(&self.active, &self.picture, &self.button, &self.inhibitor);
        }
    }

    fn test_pipeline(description: &str) -> gst::Pipeline {
        gst::parse::launch(description)
            .unwrap()
            .dynamic_cast::<gst::Pipeline>()
            .unwrap()
    }

    fn hosts_fds() -> usize {
        let hosts = std::fs::canonicalize("/etc/hosts").unwrap();
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                std::fs::read_link(entry.path()).ok().as_deref() == Some(hosts.as_path())
            })
            .count()
    }

    fn dispatch_until(mut done: impl FnMut() -> bool) {
        let context = glib::MainContext::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !done() && std::time::Instant::now() < deadline {
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            done(),
            "pipeline callback did not arrive before the deadline"
        );
    }

    #[cfg(feature = "background-throttle")]
    #[test]
    #[ignore = "requires a GTK display; never presents a window or opens a capture device"]
    fn pipeline_queued_playing_message_does_not_inhibit_a_paused_graph() {
        gtk::test_synced(|| {
            let ui = PipelineUi::new();
            let pipeline =
                test_pipeline("videotestsrc is-live=true ! fakesink async=false sync=false");
            start_pipeline(
                Path::new("/etc/hosts"),
                pipeline.clone(),
                &ui.active,
                (&ui.picture, &ui.message, &ui.button, &ui.inhibitor),
                gst::State::Paused,
            )
            .unwrap();
            // A resume may still be pending when a prior Playing message arrives.
            ui.active
                .borrow()
                .as_ref()
                .unwrap()
                .requested_state
                .set(gst::State::Playing);
            pipeline
                .bus()
                .unwrap()
                .post(
                    gst::message::StateChanged::builder(
                        gst::State::Paused,
                        gst::State::Playing,
                        gst::State::VoidPending,
                    )
                    .src(&pipeline)
                    .build(),
                )
                .unwrap();
            let done = Rc::new(Cell::new(false));
            let dispatched = done.clone();
            glib::idle_add_local_once(move || dispatched.set(true));
            dispatch_until(|| done.get());
            assert_eq!(pipeline.current_state(), gst::State::Paused);
            assert!(
                !ui.inhibitor.state.borrow().0,
                "a queued message is not proof of actual Playing"
            );
        });
    }

    #[cfg(feature = "background-throttle")]
    #[test]
    #[ignore = "requires a GTK display; never presents a window or opens a capture device"]
    fn pipeline_started_hidden_stays_paused_until_map() {
        gtk::test_synced(|| {
            let ui = PipelineUi::new();
            assert!(!ui._window.is_mapped());
            assert!(
                ui.active.borrow().is_none(),
                "nothing allocated before initial map/start"
            );
            let pipeline =
                test_pipeline("videotestsrc is-live=true ! fakesink async=false sync=false");
            start_pipeline(
                Path::new("/etc/hosts"),
                pipeline.clone(),
                &ui.active,
                (&ui.picture, &ui.message, &ui.button, &ui.inhibitor),
                capture_state_for_window(&ui._window),
            )
            .unwrap();
            assert_eq!(
                pipeline.current_state(),
                gst::State::Paused,
                "hidden Start/hotplug must not enter Playing"
            );
            assert!(!ui.inhibitor.state.borrow().0);
            assert_eq!(ui.button.label().as_deref(), Some("Stop"));
            assert!(sync_capture_visibility(
                &ui.active,
                CapturePreference::Automatic,
                true,
                gtk::gdk::ToplevelState::empty(),
                (&ui.picture, &ui.message, Some(&ui.button), &ui.inhibitor),
            )
            .unwrap());
            dispatch_until(|| {
                pipeline.current_state() == gst::State::Playing && ui.inhibitor.state.borrow().0
            });
        });
    }

    #[cfg(feature = "background-throttle")]
    #[test]
    #[ignore = "requires a GTK display; never presents a window or opens a capture device"]
    fn pipeline_background_pause_retains_graph_and_stops_frames() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        gtk::test_synced(|| {
            let ui = PipelineUi::new();
            let pipeline = test_pipeline("videotestsrc is-live=true ! video/x-raw,framerate=60/1 ! fakesink name=frames signal-handoffs=true sync=false");
            let frames = Arc::new(AtomicUsize::new(0));
            let counter = frames.clone();
            pipeline
                .by_name("frames")
                .unwrap()
                .connect("handoff", false, move |_| {
                    counter.fetch_add(1, Ordering::Relaxed);
                    None
                });
            ui.start(&pipeline).unwrap();
            dispatch_until(|| frames.load(Ordering::Relaxed) >= 3);
            let picture = ui.picture.paintable();
            let widgets = (&ui.picture, &ui.message, Some(&ui.button), &ui.inhibitor);
            assert!(sync_capture_visibility(
                &ui.active,
                CapturePreference::Automatic,
                false,
                gtk::gdk::ToplevelState::empty(),
                widgets
            )
            .unwrap());
            let (_, current, _) = pipeline.state(gst::ClockTime::from_seconds(2));
            assert_eq!(current, gst::State::Paused);
            let paused = frames.load(Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(100));
            assert_eq!(
                frames.load(Ordering::Relaxed),
                paused,
                "Paused must stop live frame production"
            );
            assert!(ui
                .active
                .borrow()
                .as_ref()
                .is_some_and(|running| running.pipeline == pipeline
                    && running.path == Path::new("/etc/hosts")));
            assert_eq!(ui.picture.paintable(), picture);
            assert_eq!(ui.button.label().as_deref(), Some("Stop"));
            assert!(
                !ui.inhibitor.state.borrow().0,
                "allocated Paused is not actual Playing"
            );
            assert!(
                !sync_capture_visibility(
                    &ui.active,
                    CapturePreference::Automatic,
                    false,
                    gtk::gdk::ToplevelState::empty(),
                    widgets
                )
                .unwrap(),
                "duplicate notification must not request a state again"
            );
            assert!(sync_capture_visibility(
                &ui.active,
                CapturePreference::Automatic,
                true,
                gtk::gdk::ToplevelState::empty(),
                widgets
            )
            .unwrap());
            dispatch_until(|| {
                frames.load(Ordering::Relaxed) >= paused + 3 && ui.inhibitor.state.borrow().0
            });
            stop(&ui.active, &ui.picture, &ui.button, &ui.inhibitor);
            assert!(!sync_capture_visibility(
                &ui.active,
                CapturePreference::Paused,
                true,
                gtk::gdk::ToplevelState::empty(),
                widgets
            )
            .unwrap());
            assert!(
                ui.active.borrow().is_none(),
                "visibility must never override manual Stop"
            );
            assert_eq!(pipeline.current_state(), gst::State::Null);
        });
    }

    #[test]
    #[ignore = "requires a GTK display; never presents a window or opens a capture device"]
    fn pipeline_watch_does_not_retain_capture_through_start_button() {
        gtk::test_synced(|| {
            let ui = PipelineUi::new();
            let active: ActiveCapture = Rc::new(RefCell::new(None));
            let weak = Rc::downgrade(&active);
            let button = gtk::Button::with_label("Start");
            let owned = active.clone();
            button.connect_clicked(move |_| {
                let _ = owned.borrow();
            });
            let pipeline =
                test_pipeline("videotestsrc is-live=true ! fakesink async=false sync=false");
            start_pipeline(
                Path::new("/etc/hosts"),
                pipeline.clone(),
                &active,
                (&ui.picture, &ui.message, &button, &ui.inhibitor),
                gst::State::Paused,
            )
            .unwrap();
            drop(button);
            drop(active);
            let retained = weak.upgrade();
            // Always clean up the RED fixture before asserting the ownership bug.
            if let Some(active) = &retained {
                stop(active, &ui.picture, &ui.button, &ui.inhibitor);
            }
            pipeline.set_state(gst::State::Null).unwrap();
            assert!(
                retained.is_none(),
                "watch -> button -> clicked -> ActiveCapture is a cycle"
            );
        });
    }

    #[test]
    #[ignore = "requires a GTK display; never presents a window or opens a capture device"]
    fn pipeline_async_error_retires_resources_without_start_button() {
        gtk::test_synced(|| {
            gst::init().unwrap();
            let window = gtk::ApplicationWindow::builder().build();
            let active: ActiveCapture = Rc::new(RefCell::new(None));
            let picture = gtk::Picture::new();
            let message = gtk::Label::new(None);
            let button = gtk::Button::with_label("Start");
            let weak_button = button.downgrade();
            let inhibitor = Rc::new(InhibitController::new(&window));
            let paintable = gtk::gdk::MemoryTexture::new(
                1,
                1,
                gtk::gdk::MemoryFormat::R8g8b8a8,
                &glib::Bytes::from_static(&[0, 0, 0, 255]),
                4,
            );
            picture.set_paintable(Some(&paintable));
            let before = hosts_fds();
            let pipeline = test_pipeline(
                "filesrc location=/etc/hosts ! identity error-after=1 ! fakesink async=false sync=false",
            );
            start_pipeline(
                Path::new("/etc/hosts"),
                pipeline.clone(),
                &active,
                (&picture, &message, &button, &inhibitor),
                gst::State::Playing,
            )
            .unwrap();
            assert_eq!(hosts_fds(), before + 1, "filesrc must really hold its FD");
            dispatch_until(|| message.text().starts_with("Capture error:"));
            assert!(active.borrow().is_some(), "retirement must still be queued");
            drop(button);
            let button_retained = weak_button.upgrade().is_some();
            let dispatched = Rc::new(Cell::new(false));
            let done = dispatched.clone();
            glib::idle_add_local_once(move || done.set(true));
            dispatch_until(|| dispatched.get());
            let current = pipeline.current_state();
            let retained_fds = hosts_fds() - before;
            // Inspect cleanup before ensuring the fixture is harmless even on RED.
            pipeline.set_state(gst::State::Null).unwrap();
            assert!(
                !button_retained,
                "Start must be gone before retirement runs"
            );
            assert!(
                active.borrow().is_none(),
                "retirement must not require Start"
            );
            assert_eq!(current, gst::State::Null);
            assert_eq!(retained_fds, 0);
            assert!(picture.paintable().is_none());
            assert!(!inhibitor.state.borrow().0);
            assert!(message.is_visible());
            assert!(pipeline
                .bus()
                .unwrap()
                .add_watch_local(|_, _| glib::ControlFlow::Continue)
                .is_ok());
        });
    }

    #[test]
    #[ignore = "requires a GTK display; never presents a window or opens a capture device"]
    fn pipeline_callbacks_do_not_retain_start_button_or_capture() {
        gtk::test_synced(|| {
            gst::init().unwrap();
            let mut retained = Vec::new();
            for error_dispatched in [false, true] {
                let window = gtk::ApplicationWindow::builder().build();
                let active: ActiveCapture = Rc::new(RefCell::new(None));
                let picture = gtk::Picture::new();
                let message = gtk::Label::new(None);
                let button = gtk::Button::with_label("Start");
                let inhibitor = Rc::new(InhibitController::new(&window));
                let pipeline = test_pipeline(if error_dispatched {
                    "filesrc location=/etc/hosts ! identity error-after=1 ! fakesink async=false sync=false"
                } else {
                    "filesrc location=/etc/hosts ! fakesink async=false sync=false"
                });
                let weak_active = Rc::downgrade(&active);
                let weak_button = button.downgrade();
                {
                    let (active, picture, message, inhibitor, pipeline) = (
                        active.clone(),
                        picture.clone(),
                        message.clone(),
                        inhibitor.clone(),
                        pipeline.clone(),
                    );
                    // Model Start's strong state ownership, without its separate
                    // pre-existing button self-capture cycle: use the signal argument.
                    button.connect_clicked(move |button| {
                        if active.borrow().is_some() {
                            stop(&active, &picture, button, &inhibitor);
                        } else {
                            start_pipeline(
                                Path::new("/etc/hosts"),
                                pipeline.clone(),
                                &active,
                                (&picture, &message, button, &inhibitor),
                                gst::State::Playing,
                            )
                            .unwrap();
                        }
                    });
                }
                button.emit_clicked();
                if error_dispatched {
                    dispatch_until(|| message.text().starts_with("Capture error:"));
                    assert!(active.borrow().is_some(), "retirement must still be queued");
                }
                drop(button);
                drop(active);
                let button_retained = weak_button.upgrade().is_some();
                let active_retained = weak_active.upgrade().is_some();
                eprintln!(
                    "error dispatched={error_dispatched}: button retained={button_retained}, capture retained={active_retained}"
                );
                retained.push((error_dispatched, button_retained, active_retained));
                // Observe actual lifetime before breaking the cycle on RED.
                if let (Some(active), Some(button)) = (weak_active.upgrade(), weak_button.upgrade())
                {
                    stop(&active, &picture, &button, &inhibitor);
                }
                pipeline.set_state(gst::State::Null).unwrap();
                let dispatched = Rc::new(Cell::new(false));
                let done = dispatched.clone();
                glib::idle_add_local_once(move || done.set(true));
                dispatch_until(|| dispatched.get());
            }
            assert_eq!(
                retained,
                vec![(false, false, false), (true, false, false)],
                "neither the bus watch nor deferred retirement may retain Start and its capture state"
            );
        });
    }

    #[test]
    #[ignore = "requires a GTK display; never presents a window or opens a capture device"]
    fn pipeline_async_error_retires_resources() {
        gtk::test_synced(|| {
            let ui = PipelineUi::new();
            let before = hosts_fds();
            let pipeline = test_pipeline(
                "filesrc location=/etc/hosts ! identity error-after=1 ! fakesink async=false sync=false",
            );
            ui.start(&pipeline).unwrap();
            assert_eq!(hosts_fds(), before + 1, "filesrc must really hold its FD");
            dispatch_until(|| ui.message.text().starts_with("Capture error:"));
            // Allow a deferred retirement to run after the real bus callback returned.
            dispatch_until(|| {
                ui.active.borrow().is_none() || !glib::MainContext::default().pending()
            });
            let current = pipeline.current_state();
            let retained_fds = hosts_fds() - before;
            eprintln!("async error: state={current:?}, retained source FDs={retained_fds}");
            assert!(
                ui.active.borrow().is_none(),
                "failed capture must leave Running"
            );
            assert_eq!(current, gst::State::Null);
            assert_eq!(retained_fds, 0);
            assert!(ui.picture.paintable().is_none());
            assert_eq!(ui.button.label().as_deref(), Some("Start"));
            assert!(!ui.inhibitor.state.borrow().0);
            assert!(ui.message.is_visible());
            assert!(pipeline
                .bus()
                .unwrap()
                .add_watch_local(|_, _| glib::ControlFlow::Continue)
                .is_ok());
        });
    }

    #[test]
    #[ignore = "requires a GTK display; never presents a window or opens a capture device"]
    fn pipeline_start_failure_retires_resources() {
        gtk::test_synced(|| {
            let ui = PipelineUi::new();
            let before = hosts_fds();
            let pipeline = test_pipeline(
                "filesrc location=/etc/hosts ! fakesink async=false sync=false state-error=paused-to-playing",
            );
            let error = ui.start(&pipeline).unwrap_err();
            let current = pipeline.current_state();
            let retained_fds = hosts_fds() - before;
            // Clean up even on RED; inspect the result before the test's own teardown.
            pipeline.set_state(gst::State::Null).unwrap();
            eprintln!("start failure: state={current:?}, retained source FDs={retained_fds}");
            assert!(error.starts_with("Cannot start capture:"));
            assert_eq!(
                current,
                gst::State::Null,
                "failed start must retire its partial graph"
            );
            assert_eq!(retained_fds, 0);
            assert!(ui.active.borrow().is_none());
            assert!(ui.picture.paintable().is_none());
            assert_eq!(ui.button.label().as_deref(), Some("Start"));
            assert!(!ui.inhibitor.state.borrow().0);
            assert!(pipeline
                .bus()
                .unwrap()
                .add_watch_local(|_, _| glib::ControlFlow::Continue)
                .is_ok());
        });
    }

    #[test]
    #[ignore = "requires a GTK display; never presents a window or opens a capture device"]
    fn pipeline_stale_error_cannot_stop_restarted_capture() {
        gtk::test_synced(|| {
            let ui = PipelineUi::new();
            let before = hosts_fds();
            let failed = test_pipeline(
                "filesrc location=/etc/hosts ! identity error-after=1 ! fakesink async=false sync=false",
            );
            ui.start(&failed).unwrap();
            dispatch_until(|| ui.message.text().starts_with("Capture error:"));
            assert!(
                ui.active.borrow().is_some(),
                "retirement must be deferred outside the bus callback"
            );
            assert_eq!(
                Rc::strong_count(&ui.active),
                1,
                "the watch must not own ActiveCapture"
            );
            stop(&ui.active, &ui.picture, &ui.button, &ui.inhibitor);
            let replacement =
                test_pipeline("filesrc location=/etc/hosts ! fakesink async=false sync=false");
            ui.start(&replacement).unwrap();
            let paintable = ui.picture.paintable().unwrap();
            let dispatched = Rc::new(Cell::new(false));
            let done = dispatched.clone();
            glib::idle_add_local_once(move || done.set(true));
            dispatch_until(|| dispatched.get());
            assert!(
                ui.active
                    .borrow()
                    .as_ref()
                    .is_some_and(|running| running.pipeline == replacement),
                "an old error's deferred cleanup must not retire the replacement",
            );
            assert_eq!(failed.current_state(), gst::State::Null);
            assert_eq!(replacement.current_state(), gst::State::Playing);
            assert_eq!(hosts_fds(), before + 1);
            assert_eq!(ui.picture.paintable(), Some(paintable));
            assert_eq!(ui.button.label().as_deref(), Some("Stop"));
            assert!(ui.inhibitor.state.borrow().0);
            assert!(!ui.message.is_visible());
        });
    }

    #[test]
    #[ignore = "requires an interactive GTK display"]
    fn overlay_cursor_tracks_controls_timeout_and_recovery() {
        use super::reset_controls_timer;
        use gtk4::{glib, prelude::*};
        use std::{cell::RefCell, rc::Rc, time::Duration};
        gtk4::init().unwrap();
        let overlay = gtk4::Overlay::new();
        let controls = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        overlay.add_overlay(&controls);
        let pending = Rc::new(RefCell::new(None));
        reset_controls_timer(&controls, &overlay, &pending);
        assert!(controls.is_visible());
        assert!(overlay.cursor().is_none());
        let loop_ = glib::MainLoop::new(None, false);
        let timeout_loop = loop_.clone();
        glib::timeout_add_local_once(Duration::from_millis(3200), move || timeout_loop.quit());
        loop_.run();
        assert!(!controls.is_visible());
        assert_eq!(overlay.cursor().unwrap().name().as_deref(), Some("none"));
        reset_controls_timer(&controls, &overlay, &pending);
        assert!(controls.is_visible());
        assert!(overlay.cursor().is_none());
        if let Some(id) = pending.borrow_mut().take() {
            id.remove();
        };
    }

    #[test]
    fn cursor_is_blank_only_while_controls_are_hidden() {
        assert_eq!(cursor_name_for_controls(true), None);
        assert_eq!(cursor_name_for_controls(false), Some("none"));
    }

    #[test]
    fn video_node_directory_monitor_observes_arrival_and_removal() {
        use gtk4::{gio, glib, prelude::*};
        use std::{
            cell::Cell,
            fs,
            rc::Rc,
            time::{Duration, SystemTime, UNIX_EPOCH},
        };
        let dir = std::env::temp_dir().join(format!(
            "kvm-monitor-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let node = dir.join("video2");
        let monitor = gio::File::for_path(&dir)
            .monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
            .unwrap();
        let loop_ = glib::MainLoop::new(None, false);
        let created = Rc::new(Cell::new(false));
        let removed = Rc::new(Cell::new(false));
        {
            let (node, loop_, created, removed) = (
                node.clone(),
                loop_.clone(),
                created.clone(),
                removed.clone(),
            );
            monitor.connect_changed(move |_, file, _, event| {
                if file.path().as_deref() != Some(node.as_path()) {
                    return;
                }
                if event == gio::FileMonitorEvent::Created {
                    created.set(true);
                    fs::remove_file(&node).unwrap();
                } else if event == gio::FileMonitorEvent::Deleted {
                    removed.set(true);
                    loop_.quit();
                }
            });
        }
        fs::write(&node, b"").unwrap();
        let timeout_loop = loop_.clone();
        let timeout =
            glib::timeout_add_local_once(Duration::from_secs(3), move || timeout_loop.quit());
        loop_.run();
        if removed.get() {
            timeout.remove();
        }
        monitor.cancel();
        fs::remove_dir(&dir).unwrap();
        assert!(
            created.get() && removed.get(),
            "GIO must report video node creation and deletion"
        );
    }

    #[test]
    fn unplugging_running_device_stops_capture_but_unrelated_changes_do_not() {
        use gtk4::gio::FileMonitorEvent;
        use std::path::Path;
        let running = Path::new("/dev/video2");
        assert!(removed_running_node(
            FileMonitorEvent::Deleted,
            running,
            running
        ));
        assert!(removed_running_node(
            FileMonitorEvent::MovedOut,
            running,
            running
        ));
        assert!(!removed_running_node(
            FileMonitorEvent::Created,
            running,
            running
        ));
        assert!(!removed_running_node(
            FileMonitorEvent::Deleted,
            Path::new("/dev/video3"),
            running
        ));
    }

    #[cfg(feature = "background-throttle")]
    #[test]
    fn initial_start_is_consumed_only_on_first_map_and_manual_stop_wins() {
        let started = Cell::new(false);
        assert!(!take_initial_start(
            false,
            CapturePreference::Automatic,
            false,
            &started
        ));
        assert!(!started.get());
        assert!(take_initial_start(
            true,
            CapturePreference::Automatic,
            false,
            &started
        ));
        assert!(!take_initial_start(
            true,
            CapturePreference::Automatic,
            false,
            &started
        ));
        let stopped = Cell::new(false);
        assert!(!take_initial_start(
            true,
            CapturePreference::Paused,
            false,
            &stopped
        ));
        assert!(!take_initial_start(
            true,
            CapturePreference::Automatic,
            false,
            &stopped
        ));
    }

    #[cfg(feature = "background-throttle")]
    #[test]
    #[ignore = "requires a GTK display; never presents a window or opens a capture device"]
    fn visibility_watch_does_not_start_or_realize_before_map() {
        gtk::test_synced(|| {
            let ui = PipelineUi::new();
            let clicks = Rc::new(Cell::new(0));
            let observed = clicks.clone();
            ui.button
                .connect_clicked(move |_| observed.set(observed.get() + 1));
            watch_capture_visibility(
                &ui._window,
                &ui.active,
                &Rc::new(Cell::new(CapturePreference::Automatic)),
                (&ui.picture, &ui.message, &ui.button, &ui.inhibitor),
            );
            assert!(ui.active.borrow().is_none());
            assert!(ui._window.surface().is_none());
            assert_eq!(clicks.get(), 0);
        });
    }

    #[cfg(feature = "background-throttle")]
    #[test]
    #[ignore = "requires a GTK display; never presents a window or opens a capture device"]
    fn visibility_watch_does_not_retain_capture_through_start_button() {
        gtk::test_synced(|| {
            let ui = PipelineUi::new();
            let active: ActiveCapture = Rc::new(RefCell::new(None));
            let weak = Rc::downgrade(&active);
            let button = gtk::Button::with_label("Start");
            let owned = active.clone();
            button.connect_clicked(move |_| {
                let _ = owned.borrow();
            });
            watch_capture_visibility(
                &ui._window,
                &active,
                &Rc::new(Cell::new(CapturePreference::Automatic)),
                (&ui.picture, &ui.message, &button, &ui.inhibitor),
            );
            drop(button);
            drop(active);
            assert!(
                weak.upgrade().is_none(),
                "visibility observers must not own capture through Start"
            );
        });
    }

    #[cfg(feature = "background-throttle")]
    #[test]
    fn first_map_does_not_toggle_a_hidden_hotplug_capture() {
        let started = Cell::new(false);
        assert!(!take_initial_start(
            true,
            CapturePreference::Automatic,
            true,
            &started
        ));
        assert!(started.get());
    }

    #[cfg(feature = "background-throttle")]
    #[test]
    fn background_policy_pauses_only_for_compositor_visibility() {
        use gtk::gdk::ToplevelState as State;
        assert_eq!(
            capture_target_state(CapturePreference::Automatic, true, State::empty()),
            Some(gst::State::Playing)
        );
        assert_eq!(
            capture_target_state(CapturePreference::Automatic, true, State::FOCUSED),
            Some(gst::State::Playing)
        );
        for state in [
            State::SUSPENDED,
            State::MINIMIZED,
            State::SUSPENDED | State::FOCUSED,
        ] {
            assert_eq!(
                capture_target_state(CapturePreference::Automatic, true, state),
                Some(gst::State::Paused)
            );
        }
        assert_eq!(
            capture_target_state(CapturePreference::Automatic, false, State::empty()),
            Some(gst::State::Paused)
        );
        assert_eq!(
            capture_target_state(CapturePreference::Paused, true, State::empty()),
            None
        );
        assert_eq!(
            capture_target_state(CapturePreference::Paused, false, State::SUSPENDED),
            None
        );
    }

    #[test]
    fn auto_capture_starts_only_when_not_paused_or_already_running() {
        assert!(should_autostart(CapturePreference::Automatic, false));
        assert!(!should_autostart(CapturePreference::Automatic, true));
        assert!(!should_autostart(CapturePreference::Paused, false));
    }

    #[test]
    fn watches_only_video_device_node_events() {
        use std::path::Path;
        assert!(is_video_node(Path::new("/dev/video2")));
        assert!(!is_video_node(Path::new("/dev/video2-metadata")));
        assert!(!is_video_node(Path::new("/dev/video")));
        assert!(!is_video_node(Path::new("/dev/input/event0")));
    }

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
