//! Optional, explicitly associated HDMI audio; never a default-input recorder.

use crate::device;
use gstreamer::{self as gst, prelude::*};
use std::{fs, path::Path};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Preferences {
    pub muted: bool,
    pub percent: f64,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            muted: true,
            percent: 25.0,
        }
    }
}

impl Preferences {
    fn bounded_percent(percent: f64) -> f64 {
        if percent.is_finite() {
            percent.clamp(0.0, 100.0)
        } else {
            0.0
        }
    }

    pub fn set_percent(&mut self, percent: f64) {
        self.percent = Self::bounded_percent(percent);
        self.muted = self.percent == 0.0;
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        if !self.muted && self.percent == 0.0 {
            self.percent = 25.0;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpenError {
    NotReady,
    Unavailable(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReady => f.write_str("KVM audio capture PCM is not ready"),
            Self::Unavailable(message) => f.write_str(message),
        }
    }
}

/// Resolve only a unique capture PCM on the video's nearest supported USB device.
///
/// Re-run for each open; neither a V4L2 number nor an ALSA card number is stable.
/// `NotReady` is reserved for missing PCM enumeration, the only retryable error.
pub(crate) fn alsa_device_for_video(
    video: &Path,
    sys_video: &Path,
    sys_sound: &Path,
) -> Result<String, OpenError> {
    let fail = |message: &str| OpenError::Unavailable(message.to_owned());
    let name = video
        .file_name()
        .ok_or_else(|| fail("Invalid video device"))?;
    let usb = device::usb_device(&sys_video.join(name).join("device"))
        .ok_or_else(|| fail("Cannot associate video with a USB device"))?;
    if !device::is_supported_usb(&usb) {
        return Err(fail(
            "Audio is supported only for the USB 3188:1000 KVM module",
        ));
    }
    let entries = fs::read_dir(sys_sound)
        .map_err(|err| OpenError::Unavailable(format!("Cannot enumerate sound devices: {err}")))?;
    let mut matches = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| OpenError::Unavailable(err.to_string()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(capture) = name
            .strip_prefix("pcmC")
            .and_then(|name| name.strip_suffix('c'))
        else {
            continue;
        };
        let Some((card, pcm)) = capture.split_once('D') else {
            continue;
        };
        if card.is_empty()
            || pcm.is_empty()
            || !card.bytes().all(|byte| byte.is_ascii_digit())
            || !pcm.bytes().all(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let Ok(pcm_path) = fs::canonicalize(entry.path()) else {
            continue;
        };
        if device::usb_device(&pcm_path).as_ref() != Some(&usb) {
            continue;
        }
        // Read from the matched PCM's canonical card, not a separately resolved
        // card-number link that hotplug could already have reassigned.
        let card_path = pcm_path
            .parent()
            .ok_or_else(|| fail("Invalid ALSA capture PCM"))?;
        let id = fs::read_to_string(card_path.join("id")).map_err(|err| {
            OpenError::Unavailable(format!("Cannot read ALSA card identity: {err}"))
        })?;
        let id = id.trim();
        if id.is_empty()
            || !id.bytes().any(|byte| byte.is_ascii_alphabetic())
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(fail("Unsafe or unsupported ALSA card identity"));
        }
        matches.push(format!("hw:CARD={id},DEV={pcm}"));
    }
    match matches.as_slice() {
        [] => Err(OpenError::NotReady),
        [only] => Ok(only.clone()),
        _ => Err(fail(
            "Multiple KVM capture PCMs found; refusing ambiguous audio input",
        )),
    }
}

pub(crate) struct Playback {
    pub(crate) pipeline: gst::Pipeline,
    volume: gst::Element,
    watch: Option<gst::bus::BusWatchGuard>,
}

impl Playback {
    pub fn open(device: &str) -> Result<Self, OpenError> {
        let source = gst::ElementFactory::make("alsasrc")
            .property("device", device)
            .build()
            .map_err(|err| {
                OpenError::Unavailable(format!("ALSA audio source unavailable: {err}"))
            })?;
        let sink = gst::ElementFactory::make("autoaudiosink")
            .property("sync", true)
            .build()
            .map_err(|err| OpenError::Unavailable(format!("Audio output unavailable: {err}")))?;
        Self::with_endpoints(source, sink)
    }

    pub(crate) fn with_endpoints(
        source: gst::Element,
        sink: gst::Element,
    ) -> Result<Self, OpenError> {
        let build = |name: &str| {
            gst::ElementFactory::make(name).build().map_err(|err| {
                OpenError::Unavailable(format!("Missing audio element {name}: {err}"))
            })
        };
        let queue = gst::ElementFactory::make("queue")
            .property("max-size-time", 100_000_000u64)
            .property("max-size-buffers", 0u32)
            .property("max-size-bytes", 0u32)
            .property_from_str("leaky", "downstream")
            .build()
            .map_err(|err| OpenError::Unavailable(err.to_string()))?;
        let convert = build("audioconvert")?;
        let resample = build("audioresample")?;
        let volume = gst::ElementFactory::make("volume")
            .property("volume", 0.25f64)
            .build()
            .map_err(|err| OpenError::Unavailable(err.to_string()))?;
        let pipeline = gst::Pipeline::new();
        let elements = [&source, &queue, &convert, &resample, &volume, &sink];
        pipeline
            .add_many(elements)
            .map_err(|err| OpenError::Unavailable(err.to_string()))?;
        gst::Element::link_many(elements).map_err(|err| OpenError::Unavailable(err.to_string()))?;
        Ok(Self {
            pipeline,
            volume,
            watch: None,
        })
    }

    pub fn set_gain(&self, percent: f64) {
        self.volume
            .set_property("volume", Preferences::bounded_percent(percent) / 100.0);
    }

    pub fn install_watch<F>(&mut self, callback: F) -> Result<(), OpenError>
    where
        F: FnMut(&gst::Bus, &gst::Message) -> gtk4::glib::ControlFlow + 'static,
    {
        self.watch = Some(
            self.pipeline
                .bus()
                .expect("audio pipeline has bus")
                .add_watch_local(callback)
                .map_err(|err| OpenError::Unavailable(err.to_string()))?,
        );
        Ok(())
    }

    pub fn play(&mut self) -> Result<(), OpenError> {
        if let Err(err) = self.pipeline.set_state(gst::State::Playing) {
            self.stop();
            return Err(OpenError::Unavailable(format!(
                "Cannot start KVM audio: {err}"
            )));
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        drop(self.watch.take());
        if let Err(err) = self.pipeline.set_state(gst::State::Null) {
            eprintln!("Could not stop KVM audio: {err}");
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_sibling_modules_and_rediscovers_renumbered_pcm() {
        let fixture = Fixture::new();
        let chosen = fixture.usb("chosen", "3188", "1000");
        let sibling = fixture.usb("sibling", "3188", "1000");
        fixture.video(&chosen);
        fixture.pcm(&sibling, 1, 0, 'c', "OtherKvm");
        fixture.pcm(&chosen, 2, 0, 'p', "KvmCapture");
        assert_eq!(
            fixture.resolve(),
            Err(OpenError::NotReady),
            "matching product IDs do not establish physical association"
        );
        fixture.pcm(&chosen, 2, 0, 'c', "KvmCapture");
        assert_eq!(fixture.resolve(), Ok("hw:CARD=KvmCapture,DEV=0".to_owned()));
        fs::remove_file(fixture.0.join("sys/class/sound/pcmC2D0c")).unwrap();
        fixture.pcm(&chosen, 9, 3, 'c', "KvmCapture_1");
        assert_eq!(
            fixture.resolve(),
            Ok("hw:CARD=KvmCapture_1,DEV=3".to_owned()),
            "each open must resolve the current explicit card ID and PCM"
        );
        for id in ["", "123", "default,DEV=0", "bad id", "音声"] {
            fs::write(chosen.join("interface/sound/card9/id"), id).unwrap();
            assert!(
                matches!(fixture.resolve(), Err(OpenError::Unavailable(_))),
                "unsafe identity {id:?} must not reach ALSA"
            );
        }
    }

    #[test]
    fn reads_identity_from_matched_pcm_not_a_reused_card_number() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let kvm = fixture.usb("kvm", "3188", "1000");
        let mic = fixture.usb("webcam", "058f", "5608");
        fixture.video(&kvm);
        fixture.pcm(&kvm, 2, 0, 'c', "KvmCapture");
        fixture.pcm(&mic, 0, 0, 'c', "Microphone");
        // Simulate card-number reuse between PCM ancestry and card-ID lookup.
        let numbered_card = fixture.0.join("sys/class/sound/card2");
        fs::remove_file(&numbered_card).unwrap();
        symlink(mic.join("interface/sound/card0"), &numbered_card).unwrap();
        assert_eq!(
            fixture.resolve(),
            Ok("hw:CARD=KvmCapture,DEV=0".to_owned()),
            "the unrelated card class link must never redirect HDMI capture to a microphone"
        );
    }

    #[test]
    fn drop_returns_graph_to_null() {
        let mut playback = synthetic_playback(false);
        playback.play().unwrap();
        let graph = playback.pipeline.clone();
        graph.state(gst::ClockTime::from_seconds(2)).0.unwrap();
        assert_eq!(graph.current_state(), gst::State::Playing);
        drop(playback);
        let state = graph.current_state();
        graph.set_state(gst::State::Null).unwrap();
        assert_eq!(
            state,
            gst::State::Null,
            "an external graph reference must not retain a live stream"
        );
    }

    fn synthetic_playback(fail_transition: bool) -> Playback {
        gst::init().unwrap();
        let source = gst::ElementFactory::make("audiotestsrc")
            .property("is-live", true)
            .build()
            .unwrap();
        let mut sink = gst::ElementFactory::make("fakesink")
            .property("sync", false)
            .property("async", false);
        if fail_transition {
            sink = sink.property_from_str("state-error", "paused-to-playing");
        }
        Playback::with_endpoints(source, sink.build().unwrap()).unwrap()
    }

    #[test]
    fn failed_play_returns_graph_to_null() {
        let mut playback = synthetic_playback(true);
        let graph = playback.pipeline.clone();
        let result = playback.play();
        let state = graph.current_state();
        graph.set_state(gst::State::Null).unwrap();
        assert!(result.is_err());
        assert_eq!(
            state,
            gst::State::Null,
            "rollback must precede fixture cleanup"
        );
    }

    #[test]
    fn native_graph_uses_explicit_alsa_and_clocked_output() {
        gst::init().unwrap();
        // Construction only: no state transition or real capture/playback.
        let playback = Playback::open("hw:CARD=SyntheticOnly,DEV=2").unwrap();
        assert_eq!(playback.pipeline.current_state(), gst::State::Null);
        let elements = playback.pipeline.children();
        let find = |factory: &str| {
            elements
                .iter()
                .find(|element| element.factory().is_some_and(|f| f.name() == factory))
                .unwrap()
        };
        let source = find("alsasrc");
        assert_eq!(
            source.property::<String>("device"),
            "hw:CARD=SyntheticOnly,DEV=2"
        );
        assert!(find("autoaudiosink").property::<bool>("sync"));
        let queue = find("queue");
        assert_eq!(queue.property::<u64>("max-size-time"), 100_000_000);
        assert_eq!(queue.property::<u32>("max-size-buffers"), 0);
        assert_eq!(queue.property::<u32>("max-size-bytes"), 0);
        for factory in ["audioconvert", "audioresample", "volume"] {
            find(factory);
        }
        assert_eq!(elements.len(), 6);
        assert!(elements
            .iter()
            .all(|element| element.current_state() == gst::State::Null));
    }

    fn render_peak(percent: f64) -> f32 {
        use std::sync::{Arc, Mutex};
        gst::init().unwrap();
        let source = gst::parse::bin_from_description(
        "audiotestsrc num-buffers=1 samplesperbuffer=480 wave=square volume=1 ! audio/x-raw,format=F32LE,channels=1,rate=48000 ! identity", true
    ).unwrap().upcast::<gst::Element>();
        let sink = gst::ElementFactory::make("fakesink")
            .property("sync", false)
            .property("async", false)
            .property("signal-handoffs", true)
            .build()
            .unwrap();
        let peak = Arc::new(Mutex::new(0.0f32));
        let observed = peak.clone();
        sink.connect("handoff", false, move |values| {
            let buffer = values[1].get::<gst::Buffer>().unwrap();
            let map = buffer.map_readable().unwrap();
            for sample in map.as_slice().as_chunks::<4>().0 {
                let value = f32::from_le_bytes(*sample).abs();
                let mut maximum = observed.lock().unwrap();
                *maximum = maximum.max(value);
            }
            None
        });
        let mut playback = Playback::with_endpoints(source, sink).unwrap();
        playback.set_gain(percent);
        playback.play().unwrap();
        let event = playback.pipeline.bus().unwrap().timed_pop_filtered(
            gst::ClockTime::from_seconds(2),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        playback.stop();
        assert!(
            matches!(
                event.as_ref().map(|event| event.view()),
                Some(gst::MessageView::Eos(_))
            ),
            "synthetic audio did not reach EOS: {event:?}"
        );
        let result = *peak.lock().unwrap();
        result
    }

    #[test]
    fn applies_gain_to_real_audio_buffers() {
        for (percent, expected) in [(0.0, 0.0), (25.0, 0.25), (100.0, 1.0)] {
            assert!((render_peak(percent) - expected).abs() < 0.0001);
        }
    }

    use std::path::PathBuf;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            use std::time::{SystemTime, UNIX_EPOCH};
            let root = std::env::temp_dir().join(format!(
                "kvm-audio-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(root.join("sys/class/video4linux/video2")).unwrap();
            fs::create_dir_all(root.join("sys/class/sound")).unwrap();
            fs::create_dir_all(root.join("sys/devices/hub")).unwrap();
            fs::write(root.join("sys/devices/hub/idVendor"), "1d6b").unwrap();
            fs::write(root.join("sys/devices/hub/idProduct"), "0002").unwrap();
            Self(root)
        }
        fn usb(&self, name: &str, vendor: &str, product: &str) -> PathBuf {
            let usb = self.0.join("sys/devices/hub").join(name);
            fs::create_dir_all(usb.join("interface")).unwrap();
            fs::write(usb.join("idVendor"), vendor).unwrap();
            fs::write(usb.join("idProduct"), product).unwrap();
            usb
        }
        fn video(&self, usb: &Path) {
            std::os::unix::fs::symlink(
                usb.join("interface"),
                self.0.join("sys/class/video4linux/video2/device"),
            )
            .unwrap();
        }
        fn pcm(&self, usb: &Path, card: u32, pcm: u32, direction: char, id: &str) {
            use std::os::unix::fs::symlink;
            let card_dir = usb.join(format!("interface/sound/card{card}"));
            let name = format!("pcmC{card}D{pcm}{direction}");
            fs::create_dir_all(card_dir.join(&name)).unwrap();
            fs::write(card_dir.join("id"), id).unwrap();
            let sound = self.0.join("sys/class/sound");
            if !sound.join(format!("card{card}")).exists() {
                symlink(&card_dir, sound.join(format!("card{card}"))).unwrap();
            }
            symlink(card_dir.join(&name), sound.join(name)).unwrap();
        }
        fn resolve(&self) -> Result<String, OpenError> {
            alsa_device_for_video(
                Path::new("/dev/video2"),
                &self.0.join("sys/class/video4linux"),
                &self.0.join("sys/class/sound"),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn selects_only_unique_pcm_on_supported_video_usb() {
        let fixture = Fixture::new();
        let kvm = fixture.usb("kvm", "3188", "1000");
        let mic = fixture.usb("webcam", "058f", "5608");
        fixture.video(&kvm);
        fixture.pcm(&mic, 0, 0, 'c', "Microphone");
        fixture.pcm(&kvm, 1, 0, 'p', "Capture");
        fixture.pcm(&kvm, 1, 0, 'c', "Capture");
        assert_eq!(fixture.resolve(), Ok("hw:CARD=Capture,DEV=0".to_owned()));
        fixture.pcm(&kvm, 1, 1, 'c', "Capture");
        assert!(
            matches!(fixture.resolve(), Err(OpenError::Unavailable(message)) if message.contains("Multiple"))
        );
        fs::remove_file(fixture.0.join("sys/class/sound/pcmC1D1c")).unwrap();
        fs::write(fixture.0.join("sys/class/sound/card1/id"), "Capture,DEV=9").unwrap();
        assert!(
            matches!(fixture.resolve(), Err(OpenError::Unavailable(message)) if message.contains("identity"))
        );
        fs::write(fixture.0.join("sys/class/sound/card1/id"), "Capture").unwrap();
        fs::remove_file(fixture.0.join("sys/class/sound/pcmC1D0c")).unwrap();
        assert_eq!(fixture.resolve(), Err(OpenError::NotReady));
        fs::write(kvm.join("idVendor"), "058f").unwrap();
        assert!(
            matches!(fixture.resolve(), Err(OpenError::Unavailable(message)) if message.contains("3188:1000"))
        );
    }

    #[test]
    fn mute_preserves_gain_and_zero_unmute_restores_default() {
        let mut prefs = Preferences::default();
        prefs.toggle_mute();
        assert!(!prefs.muted);
        prefs.percent = 60.0;
        prefs.toggle_mute();
        assert!(prefs.muted);
        assert_eq!(prefs.percent, 60.0);
        prefs.percent = 0.0;
        prefs.toggle_mute();
        assert_eq!(
            prefs,
            Preferences {
                muted: false,
                percent: 25.0
            }
        );
    }

    #[test]
    fn slider_sets_bounded_gain_and_mute_intent() {
        let mut prefs = Preferences::default();
        for (input, expected, muted) in [
            (60.0, 60.0, false),
            (200.0, 100.0, false),
            (-1.0, 0.0, true),
            (f64::NAN, 0.0, true),
            (f64::INFINITY, 0.0, true),
            (1.0, 1.0, false),
            (0.0, 0.0, true),
        ] {
            prefs.set_percent(input);
            assert_eq!(prefs.percent, expected);
            assert_eq!(prefs.muted, muted);
        }
    }

    #[test]
    fn starts_muted_at_quarter_gain() {
        let prefs = Preferences::default();
        assert!(
            prefs.muted,
            "a new process must never capture audio automatically"
        );
        assert_eq!(prefs.percent, 25.0);
    }
}
