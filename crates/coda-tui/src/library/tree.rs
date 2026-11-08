//! Browse model: a directory tree relative to the music root, built
//! incrementally from scanned tracks.
//!
//! The tree owns every track once, in insertion order ([`TrackId`]); its
//! directories, the tag index ([`super::tags`]) and playlists refer to
//! tracks by id rather than holding copies.
//!
//! Only directories that contain audio (directly or below) exist, because
//! nodes are created on insert. Directories sort by name
//! case-insensitively; tracks sort by disc, track number, then file name
//! (missing numbers sort first).

use std::cmp::Ordering;
use std::path::{Component, Path, PathBuf};

use crate::model::Track;

/// A track's position in [`LibraryTree::tracks`]; stable, because the tree
/// only grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TrackId(pub u32);

impl TrackId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DirNode {
    name: String,
    dirs: Vec<DirNode>,
    tracks: Vec<TrackId>,
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

    /// This directory's own tracks, in browse order; see
    /// [`LibraryTree::track`].
    pub fn tracks(&self) -> &[TrackId] {
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

    fn insert_track(&mut self, id: TrackId, tracks: &[Track]) {
        let track = &tracks[id.index()];
        let index = self
            .tracks
            .partition_point(|t| compare_tracks(&tracks[t.index()], track) != Ordering::Greater);
        self.tracks.insert(index, id);
    }
}

#[derive(Debug, Clone)]
pub struct LibraryTree {
    root_path: PathBuf,
    root: DirNode,
    /// Every track, by [`TrackId`].
    tracks: Vec<Track>,
}

impl LibraryTree {
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            root_path,
            root: DirNode::default(),
            tracks: Vec::new(),
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
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// Every track, indexed by [`TrackId`].
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// The track `id`. Ids come from this tree, so they are always valid.
    pub fn track(&self, id: TrackId) -> &Track {
        &self.tracks[id.index()]
    }

    /// Inserts a track under its parent directory, relative to the root.
    /// Returns its id, or `None` (ignoring the track) if it is not a local
    /// file below the root or the tree is full (`u32::MAX` tracks).
    pub fn insert(&mut self, track: Track) -> Option<TrackId> {
        let dirs = track
            .local_path()
            .and_then(|p| p.strip_prefix(&self.root_path).ok())
            .and_then(relative_dirs)?;
        let id = TrackId(u32::try_from(self.tracks.len()).ok()?);
        self.tracks.push(track);
        let Self { root, tracks, .. } = self;
        let mut node = root;
        for dir in &dirs {
            node = node.child_mut(dir);
        }
        node.insert_track(id, tracks);
        Some(id)
    }

    pub fn extend(&mut self, tracks: impl IntoIterator<Item = Track>) {
        for track in tracks {
            self.insert(track);
        }
    }

    /// Calls `f` for every track in browse order: each directory's
    /// sub-directories first (recursively), then its own tracks.
    pub fn for_each_track(&self, mut f: impl FnMut(&Track)) {
        self.for_each_id(self.root(), |id| f(self.track(id)));
    }

    /// Calls `f` for the id of every track below `dir`, in browse order.
    pub fn for_each_id(&self, dir: &DirNode, mut f: impl FnMut(TrackId)) {
        fn visit(node: &DirNode, f: &mut impl FnMut(TrackId)) {
            for dir in &node.dirs {
                visit(dir, f);
            }
            node.tracks.iter().copied().for_each(&mut *f);
        }
        visit(dir, &mut f);
    }

    /// The directory at `path` (directory names from the root), if any.
    pub fn dir<S: AsRef<str>>(&self, path: &[S]) -> Option<&DirNode> {
        let mut node = &self.root;
        for name in path {
            let name = name.as_ref();
            node = node.dirs.iter().find(|d| d.name == name)?;
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

/// Case-insensitive, then exact (so the order is total).
pub(crate) fn compare_names(a: &str, b: &str) -> Ordering {
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

    fn files(tree: &LibraryTree, node: &DirNode) -> Vec<String> {
        node.tracks()
            .iter()
            .map(|id| file_name(tree.track(*id)))
            .collect()
    }

    #[test]
    fn builds_nested_directories_incrementally() {
        let mut tree = LibraryTree::new("/music".into());
        assert!(tree.is_empty());
        assert_eq!(
            tree.insert(track("/music/b/x/1.flac", None, None)),
            Some(TrackId(0))
        );
        assert!(tree.insert(track("/music/A/2.flac", None, None)).is_some());
        assert!(tree.insert(track("/music/top.mp3", None, None)).is_some());
        assert_eq!(
            tree.insert(track("/music/a2/3.flac", None, None)),
            Some(TrackId(3))
        );
        assert_eq!(tree.len(), 4);
        assert_eq!(names(tree.root()), ["A", "a2", "b"]);
        assert_eq!(files(&tree, tree.root()), ["top.mp3"]);
        assert_eq!(names(tree.dir(&["b"]).unwrap()), ["x"]);
        assert_eq!(files(&tree, tree.dir(&["b", "x"]).unwrap()), ["1.flac"]);
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
            files(&tree, tree.dir(&["al"]).unwrap()),
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
        assert_eq!(tree.insert(track("/elsewhere/a.mp3", None, None)), None);
        assert_eq!(tree.insert(track("/music/../x.mp3", None, None)), None);
        assert!(tree.is_empty());
        assert!(tree.root().is_empty());
    }
}
