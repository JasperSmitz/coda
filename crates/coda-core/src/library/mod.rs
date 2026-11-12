//! Local library discovery (scan), the browse models built from it, and
//! local browse navigation. Separate from playback: the library only
//! produces [`crate::model::Track`]s.
//!
//! [`Library`] holds everything the scan found: the folder tree (which
//! owns the tracks), the tag index (artists and albums), the `.m3u`
//! playlists and the lyrics index (when the scan reads lyrics), all
//! updated as scan events arrive. It is plain synchronous
//! data; nothing here does I/O except [`scan`] and [`m3u::read_playlist`].

pub mod browse;
pub mod m3u;
pub mod scan;
pub mod search;
pub mod tags;
pub mod tree;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::lyrics::index::{IndexedLyrics, LyricsIndex, LyricsResults};
use crate::model::Track;
use m3u::PlaylistFile;
use tags::TagIndex;
use tree::{LibraryTree, TrackId, compare_names};

/// Playlists kept from one scan; later ones are ignored (bounds memory:
/// each holds up to [`m3u::MAX_PLAYLIST_ENTRIES`] track ids).
pub const MAX_PLAYLISTS: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlaylistId(pub u32);

/// A `.m3u`/`.m3u8` playlist matched against the scanned tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPlaylist {
    pub id: PlaylistId,
    /// The file stem.
    pub name: String,
    /// The playlist file; for ordering only, never logged.
    path: PathBuf,
    /// Matched tracks in file order, duplicates kept.
    pub tracks: Vec<TrackId>,
    /// Entries that matched no scanned track (outside the music folder,
    /// not scanned, a URL).
    pub missing: usize,
    /// Only the start of the file was read.
    pub cut: bool,
}

/// Everything the scan has delivered so far.
#[derive(Debug, Clone)]
pub struct Library {
    tree: LibraryTree,
    tags: TagIndex,
    /// Scanned tracks by path, to match playlist entries.
    by_path: HashMap<PathBuf, TrackId>,
    playlists: Vec<LocalPlaylist>,
    /// Playlists A–Z by name.
    playlists_sorted: Vec<PlaylistId>,
    /// Lyrics of the tracks that have them (see [`scan::ScanOptions`]).
    lyrics: LyricsIndex<TrackId>,
}

impl Library {
    pub fn new(root: PathBuf) -> Self {
        Self {
            tree: LibraryTree::new(root),
            tags: TagIndex::default(),
            by_path: HashMap::new(),
            playlists: Vec::new(),
            playlists_sorted: Vec::new(),
            lyrics: LyricsIndex::new(),
        }
    }

    pub fn tree(&self) -> &LibraryTree {
        &self.tree
    }

    pub fn tags(&self) -> &TagIndex {
        &self.tags
    }

    pub fn root_path(&self) -> &Path {
        self.tree.root_path()
    }

    pub fn track(&self, id: TrackId) -> &Track {
        self.tree.track(id)
    }

    /// The scanned track at `path` (as the scan reported it), if any.
    pub fn track_id_by_path(&self, path: &Path) -> Option<TrackId> {
        self.by_path.get(path).copied()
    }

    pub fn len(&self) -> usize {
        self.tree.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    /// Adds scanned tracks to the tree, the tag index and the path map.
    pub fn extend(&mut self, tracks: impl IntoIterator<Item = Track>) {
        for track in tracks {
            let path = track.local_path().map(Path::to_path_buf);
            let Some(id) = self.tree.insert(track) else {
                continue;
            };
            self.tags.insert(id, self.tree.tracks());
            if let Some(path) = path {
                self.by_path.entry(path).or_insert(id);
            }
        }
    }

    /// Matches a playlist's entries with the tracks scanned so far (the
    /// scan sends playlists after every track) and adds it. `None` once
    /// [`MAX_PLAYLISTS`] are held.
    pub fn add_playlist(&mut self, file: PlaylistFile) -> Option<PlaylistId> {
        if self.playlists.len() >= MAX_PLAYLISTS {
            return None;
        }
        let mut tracks = Vec::new();
        let mut missing = file.unresolved;
        for entry in &file.entries {
            match self.by_path.get(entry) {
                Some(id) => tracks.push(*id),
                None => missing += 1,
            }
        }
        let id = PlaylistId(self.playlists.len() as u32);
        self.playlists.push(LocalPlaylist {
            id,
            name: file.name,
            path: file.path,
            tracks,
            missing,
            cut: file.cut,
        });
        let playlists = &self.playlists;
        let order = |a: PlaylistId, b: PlaylistId| {
            let (a, b) = (&playlists[a.0 as usize], &playlists[b.0 as usize]);
            compare_names(&a.name, &b.name).then_with(|| a.path.cmp(&b.path))
        };
        let at = self
            .playlists_sorted
            .partition_point(|p| order(*p, id).is_lt());
        self.playlists_sorted.insert(at, id);
        Some(id)
    }

    /// Adds lyrics the scan read, for tracks it already delivered (by
    /// path; lyrics of an unknown path are dropped). The scan sends each
    /// track's lyrics at most once.
    pub fn add_lyrics(&mut self, lyrics: impl IntoIterator<Item = (PathBuf, IndexedLyrics)>) {
        for (path, lyrics) in lyrics {
            let Some(id) = self.by_path.get(&path).copied() else {
                continue;
            };
            self.lyrics.insert(id, lyrics);
        }
    }

    pub fn lyrics(&self) -> &LyricsIndex<TrackId> {
        &self.lyrics
    }

    /// Searches the lyrics (see [`LyricsIndex::search`]): the first
    /// `limit` matches in browse order (as local search lists tracks),
    /// and the total. Left out, so no track shows twice beside a local
    /// search for the same `query`: every track that search matches
    /// ([`search::search`]'s own rule, so also tracks the scan added
    /// after it ran and those past its cap), and the tracks in `exclude`.
    pub fn search_lyrics(
        &self,
        query: &str,
        exclude: &HashSet<TrackId>,
        limit: usize,
    ) -> LyricsResults<TrackId> {
        if self.lyrics.is_empty() {
            return LyricsResults::default();
        }
        let root = self.tree.root_path();
        let needle = search::needle(query);
        let skip = |id: &TrackId| {
            exclude.contains(id)
                || (!needle.is_empty() && search::matches(self.tree.track(*id), root, &needle))
        };
        // Each track's place in browse order (one walk of the tree).
        let mut rank = vec![u32::MAX; self.tree.len()];
        let mut next = 0u32;
        self.tree.for_each_id(self.tree.root(), |id| {
            if let Some(slot) = rank.get_mut(id.index()) {
                *slot = next;
            }
            next += 1;
        });
        let place = |id: &TrackId| rank.get(id.index()).copied().unwrap_or(u32::MAX);
        self.lyrics
            .search(query, skip, limit, |a, b| place(a).cmp(&place(b)))
    }

    /// Every playlist, A–Z.
    pub fn playlists(&self) -> &[PlaylistId] {
        &self.playlists_sorted
    }

    pub fn playlist(&self, id: PlaylistId) -> &LocalPlaylist {
        &self.playlists[id.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackMetadata;
    use std::time::{Duration, Instant};

    fn local(path: &str) -> Track {
        Track::local(path.into(), TrackMetadata::default())
    }

    fn playlist(name: &str, entries: &[&str], unresolved: usize) -> PlaylistFile {
        PlaylistFile {
            path: format!("/m/{name}.m3u").into(),
            name: name.into(),
            entries: entries.iter().map(PathBuf::from).collect(),
            unresolved,
            cut: false,
        }
    }

    #[test]
    fn playlists_match_scanned_tracks_in_order_and_sort_by_name() {
        let mut library = Library::new("/m".into());
        library.extend([local("/m/a.mp3"), local("/m/x/b.mp3")]);
        let id = library
            .add_playlist(playlist(
                "road",
                &["/m/x/b.mp3", "/m/a.mp3", "/m/gone.mp3", "/m/x/b.mp3"],
                2,
            ))
            .unwrap();
        library.add_playlist(playlist("Alpha", &[], 0)).unwrap();
        library.add_playlist(playlist("beta", &[], 0)).unwrap();
        let road = library.playlist(id);
        let titles: Vec<&str> = road
            .tracks
            .iter()
            .map(|t| library.track(*t).title.as_str())
            .collect();
        assert_eq!(titles, ["b", "a", "b"]);
        assert_eq!(road.missing, 3);
        let names: Vec<&str> = library
            .playlists()
            .iter()
            .map(|p| library.playlist(*p).name.as_str())
            .collect();
        assert_eq!(names, ["Alpha", "beta", "road"]);
    }

    #[test]
    fn tracks_are_found_by_path() {
        let mut library = Library::new(PathBuf::from("/music"));
        library.extend([local("/music/a/1.flac"), local("/music/b/2.flac")]);
        let id = library
            .track_id_by_path(Path::new("/music/b/2.flac"))
            .unwrap();
        assert_eq!(
            library.track(id).local_path(),
            Some(Path::new("/music/b/2.flac"))
        );
        assert_eq!(library.track_id_by_path(Path::new("/music/c.flac")), None);
    }

    #[test]
    fn lyrics_search_leaves_out_what_local_search_lists() {
        use crate::lyrics::{index::IndexedLyrics, parse};
        let titled = |path: &str, title: &str| {
            Track::local(
                path.into(),
                TrackMetadata {
                    title: Some(title.into()),
                    ..TrackMetadata::default()
                },
            )
        };
        let mut library = Library::new("/m".into());
        library.extend([
            titled("/m/a.flac", "Here Comes the Sun"),
            titled("/m/b.flac", "Morning"),
            titled("/m/sunny/c.flac", "Evening"),
            titled("/m/d.flac", "Night"),
        ]);
        let lyrics = |text: &str| IndexedLyrics::new(&parse(text)).unwrap();
        library.add_lyrics([
            ("/m/a.flac".into(), lyrics("here comes the sun")),
            ("/m/b.flac".into(), lyrics("the sun is up")),
            ("/m/sunny/c.flac".into(), lyrics("sun goes down")),
            ("/m/d.flac".into(), lyrics("no sun at night")),
            ("/m/unknown.flac".into(), lyrics("sun")),
        ]);
        let titles = |exclude: &HashSet<TrackId>| {
            let found = library.search_lyrics(" SUN ", exclude, 50);
            let titles: Vec<String> = found
                .matches
                .iter()
                .map(|m| library.track(m.key).title.clone())
                .collect();
            (titles, found.total)
        };
        // The title match and the path match are local search's results.
        assert_eq!(
            titles(&HashSet::new()),
            (vec!["Morning".to_owned(), "Night".to_owned()], 2)
        );
        let night = library.track_id_by_path(Path::new("/m/d.flac")).unwrap();
        assert_eq!(
            titles(&HashSet::from([night])),
            (vec!["Morning".to_owned()], 1)
        );
        assert_eq!(library.lyrics().len(), 4);
    }

    #[test]
    fn playlists_are_capped() {
        let mut library = Library::new("/m".into());
        for i in 0..MAX_PLAYLISTS {
            assert!(
                library
                    .add_playlist(playlist(&format!("{i}"), &[], 0))
                    .is_some()
            );
        }
        assert_eq!(library.add_playlist(playlist("one more", &[], 0)), None);
        assert_eq!(library.playlists().len(), MAX_PLAYLISTS);
    }

    /// 50,000 synthetic tracks with realistic tags, delivered in scan
    /// batches of 200, and a 10,000-entry playlist matched against them.
    /// Prints the worst batch and the playlist time (run with `--release
    /// --nocapture` for the real cost); the assertions only catch
    /// something pathological in debug builds.
    #[test]
    fn fifty_thousand_tracks_index_without_blocking_a_frame() {
        let tracks: Vec<Track> = (0..50_000)
            .map(|i| {
                let artist = format!("Artist Number {:04}", i / 100);
                let album = format!("Album Title Of Some Length {:04}", i / 10);
                Track::local(
                    format!("/m/{artist}/{album}/{:02} Track Name {i}.flac", i % 10).into(),
                    TrackMetadata {
                        title: Some(format!("Track Name {i} (Remastered Édition)")),
                        artist: Some(artist.clone()),
                        album_artist: (i % 3 == 0).then(|| artist.to_uppercase()),
                        album: Some(album),
                        track_no: Some(i % 10),
                        disc_no: Some(1),
                        ..TrackMetadata::default()
                    },
                )
            })
            .collect();
        let paths: Vec<PathBuf> = tracks
            .iter()
            .step_by(5)
            .map(|t| t.local_path().unwrap().to_owned())
            .collect();
        let mut library = Library::new("/m".into());
        let mut worst = Duration::ZERO;
        let start = Instant::now();
        for batch in tracks.chunks(200) {
            let t = Instant::now();
            library.extend(batch.to_vec());
            worst = worst.max(t.elapsed());
        }
        let total = start.elapsed();
        println!("index 50,000 tracks: {total:?} in all, worst batch of 200 {worst:?}");
        assert_eq!(library.len(), 50_000);
        assert_eq!(library.tags().artists().len(), 500);
        assert_eq!(library.tags().albums(tags::AlbumOrder::Name).len(), 5_000);
        let file = PlaylistFile {
            path: "/m/big.m3u".into(),
            name: "big".into(),
            entries: paths.iter().cycle().take(10_000).cloned().collect(),
            unresolved: 0,
            cut: false,
        };
        let t = Instant::now();
        let id = library.add_playlist(file).unwrap();
        let matched = t.elapsed();
        println!("match a 10,000-entry playlist: {matched:?}");
        assert_eq!(library.playlist(id).tracks.len(), 10_000);
        assert_eq!(library.playlist(id).missing, 0);
        assert!(worst < Duration::from_secs(1), "{worst:?}");
        assert!(matched < Duration::from_secs(1), "{matched:?}");

        // The worst shape: one flat folder of untagged files, so every
        // track lands in the same folder and the same "Unknown album".
        let mut library = Library::new("/m".into());
        let mut worst = Duration::ZERO;
        let start = Instant::now();
        for chunk in (0..50_000).collect::<Vec<u32>>().chunks(200) {
            let batch: Vec<Track> = chunk
                .iter()
                .map(|i| {
                    local(&format!(
                        "/m/{:05} Untagged Track Name.flac",
                        (i * 7919) % 50_000
                    ))
                })
                .collect();
            let t = Instant::now();
            library.extend(batch);
            worst = worst.max(t.elapsed());
        }
        println!(
            "index 50,000 untagged tracks in one folder: {:?} in all, worst batch of 200 {worst:?}",
            start.elapsed()
        );
        assert_eq!(library.tags().albums(tags::AlbumOrder::Name).len(), 1);
        assert!(worst < Duration::from_secs(1), "{worst:?}");
    }
}
