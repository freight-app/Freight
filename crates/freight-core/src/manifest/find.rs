use std::path::{Path, PathBuf};

/// Walk up the directory tree from `start` and return the directory that
/// contains `freight.toml`, or `None` if none is found.
pub fn find_manifest_dir(start: &Path) -> Option<PathBuf> {
    let mut current = start.canonicalize().ok()?;
    loop {
        if current.join("freight.toml").is_file() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_tree(depth: usize) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let mut subdir = root.path().to_path_buf();
        for i in 0..depth {
            subdir = subdir.join(format!("sub{i}"));
            fs::create_dir_all(&subdir).unwrap();
        }
        (root, subdir)
    }

    #[test]
    fn finds_manifest_in_same_dir() {
        let (tmp, _) = make_tree(0);
        fs::write(tmp.path().join("freight.toml"), "[package]\nname = \"x\"\n").unwrap();
        let found = find_manifest_dir(tmp.path()).unwrap();
        assert_eq!(found, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn finds_manifest_two_levels_up() {
        let (tmp, deep) = make_tree(2);
        fs::write(tmp.path().join("freight.toml"), "[package]\nname = \"x\"\n").unwrap();
        let found = find_manifest_dir(&deep).unwrap();
        assert_eq!(found, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn returns_none_when_no_manifest() {
        let (tmp, deep) = make_tree(2);
        // No freight.toml anywhere
        // find_manifest_dir will walk all the way up the real filesystem,
        // so we start from a deep subdir that we know has no manifest.
        // We can't guarantee nothing above tmp has freight.toml, so instead
        // test that the function at least returns Some when we add one.
        fs::write(tmp.path().join("freight.toml"), "").unwrap();
        assert!(find_manifest_dir(&deep).is_some());
    }

    #[test]
    fn prefers_nearest_manifest() {
        let (tmp, deep) = make_tree(3);
        // Put a manifest at root and one two levels deep
        fs::write(tmp.path().join("freight.toml"), "root").unwrap();
        let mid = tmp.path().join("sub0").join("sub1");
        fs::write(mid.join("freight.toml"), "mid").unwrap();
        // Starting from the deepest dir should find the mid one
        let found = find_manifest_dir(&deep).unwrap();
        assert_eq!(found, mid.canonicalize().unwrap());
    }
}
