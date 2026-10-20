//! Opt-in diagnostic instrumentation. Not compiled into ordinary viewer builds.
//!
//! Count buffers reaching the sink, not compositor presentation or fresh HDMI
//! content. Negotiated caps describe the stream; neither caps nor this counter
//! measure end-to-end latency, total power, or zero-copy transport.
use gstreamer::{self as gst, prelude::*};
use gtk4::glib;
use std::{
    cell::Cell,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

fn frames_per_second(before: u64, after: u64, seconds: f64) -> f64 {
    after.saturating_sub(before) as f64 / seconds
}

pub fn install(pipeline: &gst::Pipeline) {
    let sink = pipeline
        .by_name("display")
        .expect("capture has display sink");
    let pad = sink.static_pad("sink").expect("display has sink pad");
    let frames = Arc::new(AtomicU64::new(0));
    let received = frames.clone();
    pad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
        received.fetch_add(1, Ordering::Relaxed);
        gst::PadProbeReturn::Ok
    });
    let weak_pipeline = pipeline.downgrade();
    let last_count = Cell::new(0);
    let last_time = Cell::new(Instant::now());
    glib::timeout_add_local(Duration::from_secs(3), move || {
        let Some(pipeline) = weak_pipeline.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let now = Instant::now();
        let count = frames.load(Ordering::Relaxed);
        let fps = frames_per_second(
            last_count.replace(count),
            count,
            now.duration_since(last_time.replace(now)).as_secs_f64(),
        );
        let caps = pad
            .current_caps()
            .map(|c| c.to_string())
            .unwrap_or_default();
        eprintln!(
            "BENCH_FRAME frames={count} fps={fps:.3} caps={caps} state={:?}",
            pipeline.current_state()
        );
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::frames_per_second;

    #[test]
    fn measures_actual_frame_delivery_not_negotiated_rate() {
        assert_eq!(frames_per_second(100, 280, 3.0), 60.0);
        assert_eq!(frames_per_second(280, 280, 3.0), 0.0);
    }
}
