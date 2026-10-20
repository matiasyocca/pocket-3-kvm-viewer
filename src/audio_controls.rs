//! Window-scoped audio intent, controls, optional playback and bounded recovery.
//!
//! Widgets and bus/timer closures reference the controller weakly. Video supplies
//! a gate combining requested and actual Playing, independent of window focus.
//! Only missing-PCM enumeration retries; failures otherwise latch until an explicit
//! enable/start/resume. Generation and bus identity protect deferred retirement.

use crate::audio::{self, OpenError, Playback, Preferences};
use gstreamer::{self as gst, prelude::*};
use gtk4::{self as gtk, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

type Opener = fn(&Path) -> Result<Playback, OpenError>;

fn open_video_audio(video: &Path) -> Result<Playback, OpenError> {
    let device = audio::alsa_device_for_video(
        video,
        Path::new("/sys/class/video4linux"),
        Path::new("/sys/class/sound"),
    )?;
    Playback::open(&device)
}

pub(crate) struct AudioControls {
    prefs: Cell<Preferences>,
    video: RefCell<Option<PathBuf>>,
    video_playing: Cell<bool>,
    playback: RefCell<Option<Playback>>,
    error: RefCell<Option<String>>,
    generation: Cell<u64>,
    retries: Cell<usize>,
    retry: RefCell<Option<glib::SourceId>>,
    updating: Cell<bool>,
    button: glib::WeakRef<gtk::Button>,
    slider: glib::WeakRef<gtk::Scale>,
    warning: glib::WeakRef<gtk::Image>,
    opener: Opener,
}

impl AudioControls {
    pub fn new() -> (gtk::Box, Rc<Self>) {
        Self::construct(open_video_audio)
    }

    fn construct(opener: Opener) -> (gtk::Box, Rc<Self>) {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let button = gtk::Button::from_icon_name("audio-volume-muted-symbolic");
        button.set_size_request(44, 44);
        let slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        slider.set_size_request(120, 44);
        slider.set_draw_value(false);
        slider.set_value(25.0);
        slider.update_property(&[
            gtk::accessible::Property::Label("KVM audio volume"),
            gtk::accessible::Property::Description(
                "Volume for this viewer only, from zero to one hundred percent",
            ),
        ]);
        let warning = gtk::Image::from_icon_name("dialog-warning-symbolic");
        warning.set_visible(false);
        root.append(&button);
        root.append(&slider);
        root.append(&warning);
        let this = Rc::new(Self {
            prefs: Cell::new(Preferences::default()),
            video: RefCell::new(None),
            video_playing: Cell::new(false),
            playback: RefCell::new(None),
            error: RefCell::new(None),
            generation: Cell::new(0),
            retries: Cell::new(0),
            retry: RefCell::new(None),
            updating: Cell::new(false),
            button: button.downgrade(),
            slider: slider.downgrade(),
            warning: warning.downgrade(),
            opener,
        });
        {
            let weak = Rc::downgrade(&this);
            button.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    let mut prefs = this.prefs.get();
                    prefs.toggle_mute();
                    this.prefs.set(prefs);
                    this.reset_attempt();
                    this.reconcile();
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            slider.connect_value_changed(move |slider| {
                if let Some(this) = weak.upgrade() {
                    if this.updating.get() {
                        return;
                    }
                    let before = this.prefs.get();
                    let mut prefs = before;
                    prefs.set_percent(slider.value());
                    this.prefs.set(prefs);
                    // Gain changes must not hammer a failed/busy source.
                    // Crossing mute/zero is an explicit new enable/disable action.
                    if before.muted != prefs.muted {
                        this.reset_attempt();
                    }
                    this.reconcile();
                }
            });
        }
        this.render();
        (root, this)
    }

    pub fn attach(self: &Rc<Self>, video: &Path, playing: bool) {
        self.detach();
        *self.video.borrow_mut() = Some(video.to_path_buf());
        self.video_playing.set(playing);
        self.reset_attempt();
        self.reconcile();
    }

    pub fn detach(&self) {
        self.video_playing.set(false);
        self.video.borrow_mut().take();
        self.reset_attempt();
        self.stop_playback();
        self.render();
    }

    pub fn set_video_playing(self: &Rc<Self>, playing: bool) {
        if self.video_playing.replace(playing) == playing {
            return;
        }
        self.reset_attempt();
        self.reconcile();
    }

    fn enabled(&self) -> bool {
        let prefs = self.prefs.get();
        self.video.borrow().is_some()
            && self.video_playing.get()
            && !prefs.muted
            && prefs.percent > 0.0
    }

    fn cancel_retry(&self) {
        if let Some(id) = self.retry.borrow_mut().take() {
            id.remove();
        }
    }

    fn reset_attempt(&self) {
        self.cancel_retry();
        self.generation.set(self.generation.get().wrapping_add(1));
        self.retries.set(0);
        self.error.borrow_mut().take();
    }

    fn stop_playback(&self) {
        let playback = self.playback.borrow_mut().take();
        drop(playback); // Drops watch before requesting Null; no RefCell borrow held.
    }

    fn reconcile(self: &Rc<Self>) {
        if !self.enabled() {
            self.cancel_retry();
            self.stop_playback();
            self.render();
            return;
        }
        if let Some(playback) = self.playback.borrow().as_ref() {
            playback.set_gain(self.prefs.get().percent);
            self.render();
            return;
        }
        if self.error.borrow().is_some() || self.retry.borrow().is_some() {
            self.render();
            return;
        }
        let video = self
            .video
            .borrow()
            .as_ref()
            .expect("enabled has video")
            .clone();
        let result = (self.opener)(&video).and_then(|mut playback| {
            playback.set_gain(self.prefs.get().percent);
            self.watch_audio(&mut playback)?;
            playback.play()?;
            Ok(playback)
        });
        match result {
            Ok(playback) => {
                *self.playback.borrow_mut() = Some(playback);
            }
            Err(OpenError::NotReady) if self.retries.get() < 3 => {
                *self.error.borrow_mut() = Some("Waiting for the KVM audio device...".to_owned());
                self.schedule_retry();
            }
            Err(err) => {
                eprintln!("KVM audio unavailable: {err}");
                *self.error.borrow_mut() = Some(format!("{err}. Mute then unmute to retry."));
            }
        }
        self.render();
    }

    fn watch_audio(self: &Rc<Self>, playback: &mut Playback) -> Result<(), OpenError> {
        let weak = Rc::downgrade(self);
        playback.install_watch(move |bus, event| {
            let error = match event.view() {
                gst::MessageView::Error(error) => Some(error.error().to_string()),
                gst::MessageView::Eos(_) => Some("KVM audio stream ended".to_owned()),
                _ => None,
            };
            if let (Some(this), Some(error)) = (weak.upgrade(), error) {
                this.defer_failure(bus, error);
            }
            glib::ControlFlow::Continue
        })
    }

    fn defer_failure(self: &Rc<Self>, bus: &gst::Bus, message: String) {
        let weak = Rc::downgrade(self);
        let bus = bus.clone();
        let generation = self.generation.get();
        glib::idle_add_local_once(move || {
            let Some(this) = weak.upgrade() else { return };
            let current = this.generation.get() == generation
                && this
                    .playback
                    .borrow()
                    .as_ref()
                    .is_some_and(|playback| playback.pipeline.bus().as_ref() == Some(&bus));
            if !current {
                return;
            }
            this.cancel_retry();
            this.stop_playback();
            eprintln!("KVM audio error: {message}");
            *this.error.borrow_mut() = Some(format!("{message}. Mute then unmute to retry."));
            this.render();
        });
    }

    fn schedule_retry(self: &Rc<Self>) {
        let delays = [250u64, 500, 1000];
        let attempt = self.retries.get();
        if attempt >= delays.len() {
            return;
        }
        self.retries.set(attempt + 1);
        let weak = Rc::downgrade(self);
        let generation = self.generation.get();
        let id = glib::timeout_add_local_once(Duration::from_millis(delays[attempt]), move || {
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            this.retry.borrow_mut().take();
            if !this.enabled() {
                return;
            }
            this.error.borrow_mut().take();
            this.reconcile();
        });
        *self.retry.borrow_mut() = Some(id);
    }

    fn render(&self) {
        let prefs = self.prefs.get();
        let bound = self.video.borrow().is_some();
        let audible = !prefs.muted && self.video_playing.get() && self.playback.borrow().is_some();
        let action = if prefs.muted {
            "Unmute KVM audio"
        } else {
            "Mute KVM audio"
        };
        let error = self.error.borrow().clone();
        if let Some(button) = self.button.upgrade() {
            let icon = if !audible {
                "audio-volume-muted-symbolic"
            } else if prefs.percent <= 33.0 {
                "audio-volume-low-symbolic"
            } else if prefs.percent <= 66.0 {
                "audio-volume-medium-symbolic"
            } else {
                "audio-volume-high-symbolic"
            };
            button.set_icon_name(icon);
            button.set_sensitive(bound);
            button.update_property(&[gtk::accessible::Property::Label(action)]);
            let tooltip = if !bound {
                "Start video to enable KVM audio".to_owned()
            } else if let Some(error) = &error {
                format!("{action}. Audio unavailable: {error} Video is unaffected.")
            } else {
                action.to_owned()
            };
            button.set_tooltip_text(Some(&tooltip));
        }
        if let Some(slider) = self.slider.upgrade() {
            self.updating.set(true);
            slider.set_value(prefs.percent);
            self.updating.set(false);
            slider.set_sensitive(bound);
            slider.set_tooltip_text(Some(&format!(
                "KVM audio volume: {:.0}% (this viewer only)",
                prefs.percent
            )));
        }
        if let Some(warning) = self.warning.upgrade() {
            warning.set_visible(error.is_some());
            warning.set_tooltip_text(error.as_deref());
            if let Some(error) = &error {
                warning.update_property(&[gtk::accessible::Property::Label(error)]);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_opener(opener: Opener) -> (gtk::Box, Rc<Self>) {
        Self::construct(opener)
    }

    #[cfg(test)]
    pub(crate) fn current_pipeline(&self) -> Option<gst::Pipeline> {
        self.playback
            .borrow()
            .as_ref()
            .map(|playback| playback.pipeline.clone())
    }
}

impl Drop for AudioControls {
    fn drop(&mut self) {
        if let Some(id) = self.retry.get_mut().take() {
            id.remove();
        }
        drop(self.playback.get_mut().take());
    }
}

#[cfg(test)]
pub(crate) fn test_open(_: &Path) -> Result<Playback, OpenError> {
    gst::init().unwrap();
    let source = gst::ElementFactory::make("audiotestsrc")
        .property("is-live", true)
        .build()
        .unwrap();
    let sink = gst::ElementFactory::make("fakesink")
        .property("sync", false)
        .property("async", false)
        .build()
        .unwrap();
    Playback::with_endpoints(source, sink)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires GTK_A11Y=test and a GTK display; widget metadata and fake busy input only"]
    fn audio_controls_have_touch_targets_and_local_warning() {
        gtk::test_synced(|| {
            let (widgets, control) = AudioControls::new_with_opener(busy_open);
            let button = control.button.upgrade().unwrap();
            let slider = control.slider.upgrade().unwrap();
            let warning = control.warning.upgrade().unwrap();
            assert_eq!(widgets.first_child(), Some(button.clone().upcast()));
            assert_eq!(button.next_sibling(), Some(slider.clone().upcast()));
            assert_eq!(slider.next_sibling(), Some(warning.clone().upcast()));
            assert!(button.width_request() >= 44 && button.height_request() >= 44);
            assert_eq!(slider.width_request(), 120);
            assert!(slider.height_request() >= 44);
            let range = slider.adjustment();
            assert_eq!(
                (range.lower(), range.upper(), range.step_increment()),
                (0.0, 100.0, 1.0)
            );
            assert_eq!(
                button.tooltip_text().as_deref(),
                Some("Start video to enable KVM audio")
            );
            assert!(!warning.is_visible());
            control.attach(Path::new("/dev/video2"), true);
            assert_eq!(button.tooltip_text().as_deref(), Some("Unmute KVM audio"));
            button.emit_clicked();
            assert!(warning.is_visible());
            assert!(warning.tooltip_text().unwrap().contains("busy"));
            assert!(button
                .tooltip_text()
                .unwrap()
                .contains("Video is unaffected"));
            #[cfg(feature = "background-throttle")]
            {
                assert!(gtk::test_accessible_has_property(
                    &button,
                    gtk::AccessibleProperty::Label
                ));
                assert!(gtk::test_accessible_has_property(
                    &slider,
                    gtk::AccessibleProperty::Label
                ));
                assert!(gtk::test_accessible_has_property(
                    &slider,
                    gtk::AccessibleProperty::Description
                ));
                assert!(gtk::test_accessible_has_property(
                    &warning,
                    gtk::AccessibleProperty::Label
                ));
            }
            control.detach();
        });
    }

    #[test]
    #[ignore = "requires a GTK display; fake enumeration and synthetic audio only"]
    fn mute_and_pause_cancel_pcm_retries_without_losing_intent() {
        gtk::test_synced(|| {
            FAILED_OPENS.store(0, Ordering::SeqCst);
            let (_widgets, control) = AudioControls::new_with_opener(forever_pending);
            control.attach(Path::new("/dev/video2"), true);
            let button = control.button.upgrade().unwrap();
            button.emit_clicked();
            assert!(control.retry.borrow().is_some());
            button.emit_clicked();
            assert!(control.retry.borrow().is_none());
            dispatch_for(Duration::from_millis(350));
            assert_eq!(FAILED_OPENS.load(Ordering::SeqCst), 1);
            assert!(control.prefs.get().muted);
            button.emit_clicked();
            assert!(control.retry.borrow().is_some());
            control.set_video_playing(false);
            assert!(control.retry.borrow().is_none());
            dispatch_for(Duration::from_millis(350));
            assert_eq!(FAILED_OPENS.load(Ordering::SeqCst), 2);
            assert!(!control.prefs.get().muted);
            assert_eq!(control.prefs.get().percent, 25.0);
            control.set_video_playing(true);
            assert_eq!(FAILED_OPENS.load(Ordering::SeqCst), 3);
            assert!(control.retry.borrow().is_some());
            control.detach();
        });
    }

    #[test]
    #[ignore = "requires a GTK display; verifies real watch/drop with synthetic endpoints"]
    fn widgets_bus_and_retry_do_not_retain_controller() {
        gtk::test_synced(|| {
            for mode in [0, 1, 2] {
                let (_widgets, control) = AudioControls::new_with_opener(if mode == 2 {
                    forever_pending
                } else {
                    test_open
                });
                control.attach(Path::new("/dev/video2"), true);
                let button = control.button.upgrade().unwrap();
                button.emit_clicked();
                let graph = control.current_pipeline();
                if mode == 1 {
                    control.defer_failure(
                        &graph.as_ref().unwrap().bus().unwrap(),
                        "queued old error".to_owned(),
                    );
                }
                let weak = Rc::downgrade(&control);
                drop(control);
                assert!(
                    weak.upgrade().is_none(),
                    "neither widgets, bus nor timer may own the controller"
                );
                if let Some(graph) = graph {
                    assert_eq!(graph.current_state(), gst::State::Null);
                    let watch = graph
                        .bus()
                        .unwrap()
                        .add_watch_local(|_, _| glib::ControlFlow::Continue)
                        .unwrap();
                    drop(watch);
                }
                button.emit_clicked(); // A dead weak callback must be harmless.
                dispatch_for(Duration::from_millis(350));
            }
        });
    }

    static FAILED_OPENS: AtomicUsize = AtomicUsize::new(0);
    fn forever_pending(_: &Path) -> Result<Playback, OpenError> {
        FAILED_OPENS.fetch_add(1, Ordering::SeqCst);
        Err(OpenError::NotReady)
    }
    fn busy_open(_: &Path) -> Result<Playback, OpenError> {
        FAILED_OPENS.fetch_add(1, Ordering::SeqCst);
        Err(OpenError::Unavailable("busy".to_owned()))
    }

    #[test]
    #[ignore = "requires a GTK display; fake missing PCM only"]
    fn missing_pcm_retry_budget_is_finite() {
        gtk::test_synced(|| {
            FAILED_OPENS.store(0, Ordering::SeqCst);
            let (_widgets, control) = AudioControls::new_with_opener(forever_pending);
            control.attach(Path::new("/dev/video2"), true);
            control.button.upgrade().unwrap().emit_clicked();
            dispatch_until(|| {
                FAILED_OPENS.load(Ordering::SeqCst) == 4 && control.retry.borrow().is_none()
            });
            dispatch_for(Duration::from_millis(300));
            assert_eq!(FAILED_OPENS.load(Ordering::SeqCst), 4);
            assert!(control.current_pipeline().is_none());
            assert!(control.error.borrow().is_some());
            control.detach();
        });
    }

    #[test]
    #[ignore = "requires a GTK display; fake busy PCM only"]
    fn busy_source_does_not_retry_on_gain_changes() {
        gtk::test_synced(|| {
            FAILED_OPENS.store(0, Ordering::SeqCst);
            let (_widgets, control) = AudioControls::new_with_opener(busy_open);
            control.attach(Path::new("/dev/video2"), true);
            let button = control.button.upgrade().unwrap();
            button.emit_clicked();
            for percent in [30.0, 40.0, 50.0] {
                control.set_video_playing(true);
                control.slider.upgrade().unwrap().set_value(percent);
            }
            dispatch_for(Duration::from_millis(300));
            assert_eq!(FAILED_OPENS.load(Ordering::SeqCst), 1);
            assert!(control.current_pipeline().is_none());
            button.emit_clicked();
            button.emit_clicked();
            assert_eq!(FAILED_OPENS.load(Ordering::SeqCst), 2);
            control.detach();
        });
    }
    static SETTLING_OPENS: AtomicUsize = AtomicUsize::new(0);
    fn settling_open(path: &Path) -> Result<Playback, OpenError> {
        if SETTLING_OPENS.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(OpenError::NotReady)
        } else {
            test_open(path)
        }
    }

    #[test]
    #[ignore = "requires a GTK display; synthetic late PCM only"]
    fn missing_pcm_settles_once_and_stop_cancels_retry() {
        gtk::test_synced(|| {
            SETTLING_OPENS.store(0, Ordering::SeqCst);
            let (_widgets, control) = AudioControls::new_with_opener(settling_open);
            control.attach(Path::new("/dev/video2"), true);
            control.button.upgrade().unwrap().emit_clicked();
            dispatch_until(|| control.current_pipeline().is_some());
            assert_eq!(SETTLING_OPENS.load(Ordering::SeqCst), 2);
            control.detach();
            SETTLING_OPENS.store(0, Ordering::SeqCst);
            control.attach(Path::new("/dev/video2"), true); // retained unmute intent starts attempt
            assert!(control.retry.borrow().is_some());
            control.detach();
            dispatch_for(Duration::from_millis(350));
            assert_eq!(SETTLING_OPENS.load(Ordering::SeqCst), 1);
            assert!(control.retry.borrow().is_none());
            assert!(control.current_pipeline().is_none());
        });
    }

    fn error_open(_: &Path) -> Result<Playback, OpenError> {
        gst::init().unwrap();
        let source = gst::parse::bin_from_description(
            "audiotestsrc is-live=true ! identity error-after=2",
            true,
        )
        .unwrap()
        .upcast::<gst::Element>();
        let sink = gst::ElementFactory::make("fakesink")
            .property("sync", false)
            .property("async", false)
            .build()
            .unwrap();
        Playback::with_endpoints(source, sink)
    }

    #[test]
    #[ignore = "requires a GTK display; synthetic audio errors only"]
    fn async_failure_is_latched_and_stale_cleanup_is_ignored() {
        gtk::test_synced(|| {
            let (_widgets, failed) = AudioControls::new_with_opener(error_open);
            failed.attach(Path::new("/dev/video2"), true);
            failed.button.upgrade().unwrap().emit_clicked();
            let graph = failed.current_pipeline().unwrap();
            dispatch_until(|| failed.current_pipeline().is_none());
            assert_eq!(graph.current_state(), gst::State::Null);
            assert!(failed.error.borrow().is_some());
            failed.slider.upgrade().unwrap().set_value(55.0);
            assert!(
                failed.current_pipeline().is_none(),
                "gain must not retry a latched failure"
            );
            failed.detach();

            let (_widgets, control) = AudioControls::new_with_opener(test_open);
            control.attach(Path::new("/dev/video2"), true);
            let button = control.button.upgrade().unwrap();
            button.emit_clicked();
            let old = control.current_pipeline().unwrap();
            control.defer_failure(&old.bus().unwrap(), "stale error".to_owned());
            button.emit_clicked();
            button.emit_clicked();
            let replacement = control.current_pipeline().unwrap();
            dispatch_for(Duration::from_millis(50));
            assert_eq!(control.current_pipeline(), Some(replacement.clone()));
            assert!(control.error.borrow().is_none());
            replacement
                .state(gst::ClockTime::from_seconds(2))
                .0
                .unwrap();
            assert_eq!(replacement.current_state(), gst::State::Playing);
            control.detach();
        });
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    fn dispatch_until(mut done: impl FnMut() -> bool) {
        let context = glib::MainContext::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !done() && std::time::Instant::now() < deadline {
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(done(), "audio condition did not arrive before the deadline");
    }

    fn dispatch_for(duration: Duration) {
        let context = glib::MainContext::default();
        let deadline = std::time::Instant::now() + duration;
        while std::time::Instant::now() < deadline {
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    #[ignore = "requires a GTK display; synthetic audio only, no ALSA or speakers"]
    fn mute_slider_and_video_gate_control_real_graph() {
        gtk::test_synced(|| {
            let (_widgets, control) = AudioControls::new_with_opener(test_open);
            let button = control.button.upgrade().unwrap();
            let slider = control.slider.upgrade().unwrap();
            assert!(!button.is_sensitive());
            control.attach(Path::new("/dev/video2"), true);
            assert!(
                control.current_pipeline().is_none(),
                "fresh process must remain muted"
            );
            button.emit_clicked();
            let first = control
                .current_pipeline()
                .expect("unmute must open an audio graph");
            slider.set_value(40.0);
            assert_eq!(control.prefs.get().percent, 40.0);
            assert_eq!(
                control.current_pipeline(),
                Some(first.clone()),
                "gain must not rebuild audio"
            );
            button.emit_clicked();
            assert_eq!(first.current_state(), gst::State::Null);
            assert!(control.current_pipeline().is_none());
            assert_eq!(control.prefs.get().percent, 40.0);
            button.emit_clicked();
            let second = control.current_pipeline().unwrap();
            control.set_video_playing(false);
            assert_eq!(second.current_state(), gst::State::Null);
            assert!(control.current_pipeline().is_none());
            control.set_video_playing(true);
            assert!(control.current_pipeline().is_some());
            slider.set_value(0.0);
            assert!(control.prefs.get().muted);
            assert!(control.current_pipeline().is_none());
            slider.set_value(50.0);
            assert!(!control.prefs.get().muted);
            assert!(control.current_pipeline().is_some());
            control.detach();
            assert!(control.current_pipeline().is_none());
            assert_eq!(control.prefs.get().percent, 50.0);
        });
    }
}
