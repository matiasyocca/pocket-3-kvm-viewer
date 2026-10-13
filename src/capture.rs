pub fn valid_device(path: &str) -> bool {
    path.strip_prefix("/dev/video")
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

pub fn pipeline_description(path: &str) -> String {
    format!("v4l2src device={path} ! queue max-size-buffers=2 leaky=downstream ! videoconvert ! gtk4paintablesink name=display sync=false")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_uses_low_latency_video_path() {
        assert_eq!(
            pipeline_description("/dev/video2"),
            "v4l2src device=/dev/video2 ! queue max-size-buffers=2 leaky=downstream ! videoconvert ! gtk4paintablesink name=display sync=false"
        );
    }

    #[test]
    fn rejects_pipeline_injection_in_device_path() {
        assert!(!valid_device("/dev/video2 ! fakesink"));
        assert!(!valid_device("/tmp/video2"));
        assert!(valid_device("/dev/video23"));
    }
}
