//! Browse model: a directory tree relative to the music root, built
//! incrementally from scanned tracks.
//!
//! Only directories that contain audio (directly or below) exist, because
//! nodes are created on insert. Directories sort by name
//! case-insensitively; tracks sort by disc, track number, then file name
//! (missing numbers sort first).

use std::cmp::Ordering;
use std::path::{Component, Path, PathBuf};

use crate::model::Track;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DirNode {
    name: String,
    dirs: Vec<DirNode>,
    tracks: Vec<Track>,
}

impl DirNode {
    fn new(name: String) -> Self {
        Self {
            name,
            ..Self::default()
        }
    }

    /// Directory name (empty for the root).
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn dirs(&self) -> &[DirNode] {
        &self.dirs
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty() && self.tracks.is_empty()
    }

    fn child_mut(&mut self, name: &str) -> &mut DirNode {
        let index = match self.dirs.binary_search_by(|d| compare_names(&d.name, name)) {
            Ok(index) => index,
            Err(index) => {
                self.dirs.insert(index, DirNode::new(name.to_owned()));
                index
            }
        };
        &mut self.dirs[index]
    }

    fn insert_track(&mut self, track: Track) {
        let index = self
            .tracks
            .partition_point(|t| compare_tracks(t, &track) != Ordering::Greater);
        self.tracks.insert(index, track);
    }
}

#[derive(Debug, Clone)]
pub struct LibraryTree {
    root_path: PathBuf,
    root: DirNode,
    len: usize,
}

impl LibraryTree {
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            root_path,
            root: DirNode::default(),
            len: 0,
        }
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    pub fn root(&self) -> &DirNode {
        &self.root
    }

    /// Number of tracks in the tree.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Inserts a track under its parent directory, relative to the root.
    /// Returns `false` (and ignores the track) if it is not a local file
    /// below the root.
    pub fn insert(&mut self, track: Track) -> bool {
        let Some(dirs) = track
            .local_path()
            .and_then(|p| p.strip_prefix(&self.root_path).ok())
            .and_then(relative_dirs)
        else {
            return false;
        };
        let mut node = &mut self.root;
        for dir in &dirs {
            node = node.child_mut(dir);
        }
        node.insert_track(track);
        self.len += 1;
        true
    }

    pub fn extend(&mut self, tracks: impl IntoIterator<Item = Track>) {
        for track in tracks {
            self.insert(track);
        }
    }

    /// Calls `f` for every track in browse order: each directory's
    /// sub-directories first (recursively), then its own tracks.
    pub fn for_each_track(&self, mut f: impl FnMut(&Track)) {
        fn visit(node: &DirNode, f: &mut impl FnMut(&Track)) {
            for dir in &node.dirs {
                visit(dir, f);
            }
            node.tracks.iter().for_each(f);
        }
        visit(&self.root, &mut f);
    }

    /// The directory at `path` (directory names from the root), if any.
    pub fn dir(&self, path: &[&str]) -> Option<&DirNode> {
        let mut node = &self.root;
        for name in path {
            node = node.dirs.iter().find(|d| d.name == *name)?;
        }
        Some(node)
    }
}

/// Parent directory names of a relative file path; `None` if the path has
/// no file name or contains anything but normal components.
fn relative_dirs(relative: &Path) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            _ => return None,
        }
    }
    parts.pop()?;
    Some(parts)
}

fn compare_names(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

fn file_name(track: &Track) -> String {
    track
        .local_path()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn compare_tracks(a: &Track, b: &Track) -> Ordering {
    a.disc_no
        .cmp(&b.disc_no)
        .then_with(|| a.track_no.cmp(&b.track_no))
        .then_with(|| compare_names(&file_name(a), &file_name(b)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackMetadata;

    fn track(path: &str, disc: Option<u32>, no: Option<u32>) -> Track {
        Track::local(
            path.into(),
            TrackMetadata {
                disc_no: disc,
                track_no: no,
                ..TrackMetadata::default()
            },
        )
    }

    fn names(node: &DirNode) -> Vec<&str> {
        node.dirs().iter().map(DirNode::name).collect()
    }

    fn files(node: &DirNode) -> Vec<String> {
        node.tracks().iter().map(file_name).collect()
    }

    #[test]
    fn builds_nested_directories_incrementally() {
        let mut tree = LibraryTree::new("/music".into());
        assert!(tree.is_empty());
        assert!(tree.insert(track("/music/b/x/1.flac", None, None)));
        assert!(tree.insert(track("/music/A/2.flac", None, None)));
        assert!(tree.insert(track("/music/top.mp3", None, None)));
        assert!(tree.insert(track("/music/a2/3.flac", None, None)));
        assert_eq!(tree.len(), 4);
        assert_eq!(names(tree.root()), ["A", "a2", "b"]);
        assert_eq!(files(tree.root()), ["top.mp3"]);
        assert_eq!(names(tree.dir(&["b"]).unwrap()), ["x"]);
        assert_eq!(files(tree.dir(&["b", "x"]).unwrap()), ["1.flac"]);
        assert!(tree.dir(&["missing"]).is_none());
    }

    #[test]
    fn directories_sort_case_insensitively() {
        let mut tree = LibraryTree::new("/m".into());
        for d in ["beta", "Alpha", "gamma", "Beta"] {
            tree.insert(track(&format!("/m/{d}/t.mp3"), None, None));
        }
        assert_eq!(names(tree.root()), ["Alpha", "Beta", "beta", "gamma"]);
    }

    #[test]
    fn tracks_sort_by_disc_number_then_file_name() {
        let mut tree = LibraryTree::new("/m".into());
        tree.extend([
            track("/m/al/z.flac", Some(2), Some(1)),
            track("/m/al/y.flac", Some(1), Some(2)),
            track("/m/al/x.flac", Some(1), Some(1)),
            track("/m/al/B.flac", None, None),
            track("/m/al/a.flac", None, None),
            track("/m/al/w.flac", Some(1), Some(2)),
        ]);
        assert_eq!(
            files(tree.dir(&["al"]).unwrap()),
            ["a.flac", "B.flac", "x.flac", "w.flac", "y.flac", "z.flac"]
        );
    }

    #[test]
    fn tracks_are_visited_in_browse_order() {
        let mut tree = LibraryTree::new("/m".into());
        tree.extend([
            track("/m/top.mp3", None, None),
            track("/m/b/2.mp3", None, None),
            track("/m/a/x/1.mp3", None, None),
            track("/m/a/0.mp3", None, None),
        ]);
        let mut seen = Vec::new();
        tree.for_each_track(|t| seen.push(t.local_path().unwrap().to_owned()));
        let expected: Vec<PathBuf> = ["/m/a/x/1.mp3", "/m/a/0.mp3", "/m/b/2.mp3", "/m/top.mp3"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(seen, expected);
    }

    #[test]
    fn tracks_outside_root_are_rejected() {
        let mut tree = LibraryTree::new("/music".into());
        assert!(!tree.insert(track("/elsewhere/a.mp3", None, None)));
        assert!(!tree.insert(track("/music/../x.mp3", None, None)));
        assert!(tree.is_empty());
        assert!(tree.root().is_empty());
    }
}
