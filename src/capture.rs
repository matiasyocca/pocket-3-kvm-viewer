pub fn valid_device(path: &str) -> bool {
    path.strip_prefix("/dev/video")
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

pub fn pipeline_description(path: &str) -> String {
    format!("v4l2src device={path} ! queue max-size-buffers=2 leaky=downstream ! videoconvert ! gtk4paintablesink name=display sync=false")
}

pub fn parse_args(args: &[String]) -> Result<Option<String>, &'static str> {
    match args {
        [_] => Ok(None),
        [_, flag, device] if flag == "--device" && valid_device(device) => Ok(Some(device.clone())),
        _ => Err("Usage: pocket3-kvm-viewer [--device /dev/videoN]"),
    }
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

    #[test]
    fn accepts_optional_explicit_device_only() {
        assert_eq!(parse_args(&["app".into()]).unwrap(), None);
        assert_eq!(
            parse_args(&["app".into(), "--device".into(), "/dev/video2".into()]).unwrap(),
            Some("/dev/video2".to_string())
        );
        assert!(parse_args(&["app".into(), "--device".into(), "/tmp/video2".into()]).is_err());
        assert!(parse_args(&["app".into(), "--device".into()]).is_err());
        assert_eq!(
            parse_args(&["app".into(), "--unknown".into()]),
            Err("Usage: pocket3-kvm-viewer [--device /dev/videoN]")
        );
    }
}
