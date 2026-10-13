use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub fn list_video_nodes(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut nodes: Vec<(u32, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(suffix) = name.strip_prefix("video") else {
            continue;
        };
        if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        if let Ok(index) = suffix.parse::<u32>() {
            nodes.push((index, entry.path()));
        }
    }
    nodes.sort_by_key(|(index, _)| *index);
    Ok(nodes.into_iter().map(|(_, path)| path).collect())
}

const KVM_USB_VENDOR: &str = "3188";
const KVM_USB_PRODUCT: &str = "1000";

fn is_kvm_capture_node(sys_video: &Path) -> bool {
    if !fs::read_to_string(sys_video.join("index")).is_ok_and(|index| index.trim() == "0") {
        return false;
    }
    let Ok(device) = fs::canonicalize(sys_video.join("device")) else {
        return false;
    };
    device.ancestors().any(|parent| {
        matches!(
            (fs::read_to_string(parent.join("idVendor")), fs::read_to_string(parent.join("idProduct"))),
            (Ok(vendor), Ok(product))
                if vendor.trim().eq_ignore_ascii_case(KVM_USB_VENDOR)
                && product.trim().eq_ignore_ascii_case(KVM_USB_PRODUCT)
        )
    })
}

pub fn list_kvm_nodes(dev_dir: &Path, sys_video_dir: &Path) -> io::Result<Vec<PathBuf>> {
    Ok(list_video_nodes(dev_dir)?
        .into_iter()
        .filter(|node| {
            node.file_name()
                .is_some_and(|name| is_kvm_capture_node(&sys_video_dir.join(name)))
        })
        .collect())
}

pub fn choose_kvm_node(nodes: &[PathBuf]) -> Result<PathBuf, &'static str> {
    match nodes {
        [] => Err("No KVM capture device found. Attach the module and tap Start."),
        [node] => Ok(node.clone()),
        _ => Err("Multiple KVM capture devices found. Use --device /dev/videoN."),
    }
}

pub fn resolve_capture_node(
    dev_dir: &Path,
    sys_video_dir: &Path,
    explicit: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return list_video_nodes(dev_dir)
            .map_err(|err| err.to_string())?
            .into_iter()
            .find(|node| node == Path::new(path))
            .ok_or_else(|| {
                format!("Video node {path} is not present. Tap Start after attaching it.")
            });
    }
    let nodes = list_kvm_nodes(dev_dir, sys_video_dir)
        .map_err(|err| format!("Could not search video devices: {err}"))?;
    choose_kvm_node(&nodes).map_err(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn lists_only_numeric_video_nodes_in_numeric_order() {
        let dir = std::env::temp_dir().join(format!(
            "pocket3-kvm-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        for name in ["video10", "video2", "video0", "videofoo", "other"] {
            fs::write(dir.join(name), b"").unwrap();
        }
        let result = list_video_nodes(&dir).unwrap();
        assert_eq!(
            result,
            vec![dir.join("video0"), dir.join("video2"), dir.join("video10")]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn filters_by_usb_id_and_excludes_metadata_node() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "pocket3-kvm-id-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dev = root.join("dev");
        let sys = root.join("sys/class/video4linux");
        fs::create_dir_all(&dev).unwrap();
        for (node, vendor, product, index) in [
            ("video0", "058f", "5608", "0"),
            ("video2", "3188", "1000", "0"),
            ("video3", "3188", "1000", "1"),
            ("video4", "3188", "1001", "0"),
        ] {
            fs::write(dev.join(node), b"").unwrap();
            let usb = root.join(format!("sys/devices/{node}/usb/interface"));
            fs::create_dir_all(&usb).unwrap();
            fs::write(usb.parent().unwrap().join("idVendor"), vendor).unwrap();
            fs::write(usb.parent().unwrap().join("idProduct"), product).unwrap();
            let video = sys.join(node);
            fs::create_dir_all(&video).unwrap();
            fs::write(video.join("index"), index).unwrap();
            symlink(&usb, video.join("device")).unwrap();
        }
        assert_eq!(
            list_kvm_nodes(&dev, &sys).unwrap(),
            vec![dev.join("video2")]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automatically_uses_only_a_unique_kvm_capture_node() {
        let one = vec![PathBuf::from("/dev/video2")];
        let many = vec![PathBuf::from("/dev/video2"), PathBuf::from("/dev/video4")];
        assert_eq!(choose_kvm_node(&one), Ok(PathBuf::from("/dev/video2")));
        assert!(choose_kvm_node(&[]).unwrap_err().contains("No KVM"));
        assert!(choose_kvm_node(&many).unwrap_err().contains("Multiple"));
    }

    #[test]
    fn rediscovers_capture_node_after_hotplug() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "pocket3-kvm-retry-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dev = root.join("dev");
        let sys = root.join("sys/class/video4linux");
        fs::create_dir_all(&dev).unwrap();
        assert!(resolve_capture_node(&dev, &sys, None).is_err());

        let usb = root.join("sys/devices/usb4/4-2/4-2:1.0");
        fs::create_dir_all(&usb).unwrap();
        fs::write(usb.parent().unwrap().join("idVendor"), "3188\n").unwrap();
        fs::write(usb.parent().unwrap().join("idProduct"), "1000\n").unwrap();
        let video = sys.join("video2");
        fs::create_dir_all(&video).unwrap();
        fs::write(video.join("index"), "0\n").unwrap();
        symlink(&usb, video.join("device")).unwrap();
        fs::write(dev.join("video2"), b"").unwrap();

        assert_eq!(
            resolve_capture_node(&dev, &sys, None).unwrap(),
            dev.join("video2")
        );
        assert_eq!(
            resolve_capture_node(&dev, &sys, dev.join("video2").to_str()).unwrap(),
            dev.join("video2")
        );
        assert!(resolve_capture_node(&dev, &sys, Some("/dev/video9")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires an attached KVM module and KVM_EXPECTED_DEVICE"]
    fn finds_attached_kvm_capture_node() {
        let expected = std::env::var("KVM_EXPECTED_DEVICE").unwrap();
        let nodes = list_kvm_nodes(Path::new("/dev"), Path::new("/sys/class/video4linux")).unwrap();
        assert!(
            nodes.contains(&PathBuf::from(&expected)),
            "found: {nodes:?}"
        );
    }
}
