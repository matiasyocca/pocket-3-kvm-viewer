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
}
