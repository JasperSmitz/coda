//! Local search: a case-insensitive substring match over the scanned
//! tracks. A linear scan without an index: with 50,000 tracks a search
//! that matches nothing (the worst case) took about 14 ms in a release
//! build on an Apple-silicon Mac (see the test below), so it runs
//! synchronously when a search is submitted. The cost is dominated by
//! visiting each track's heap-allocated strings, not by lowercasing (an
//! allocation-free ASCII path measured no faster).

use std::path::Path;

use super::tree::LibraryTree;
use crate::model::Track;

/// Upper bound on the results kept (and shown); the total is still
/// counted.
pub const MAX_RESULTS: usize = 1_000;

/// The outcome of one local search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResults {
    /// The trimmed query, as typed (for the title).
    pub query: String,
    /// The first [`MAX_RESULTS`] matches, in browse order.
    pub tracks: Vec<Track>,
    /// Every match, including those beyond [`MAX_RESULTS`].
    pub total: usize,
}

/// Searches every track in `tree` for `query` (trimmed), matching title,
/// artist, album and the path relative to the music root, ignoring case.
/// `None` for a blank query.
pub fn search(tree: &LibraryTree, query: &str) -> Option<SearchResults> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let needle = query.to_lowercase();
    let root = tree.root_path();
    let mut tracks = Vec::new();
    let mut total = 0;
    tree.for_each_track(|track| {
        if matches(track, root, &needle) {
            total += 1;
            if tracks.len() < MAX_RESULTS {
                tracks.push(track.clone());
            }
        }
    });
    Some(SearchResults {
        query: query.to_owned(),
        tracks,
        total,
    })
}

/// `needle` is already lowercase.
fn matches(track: &Track, root: &Path, needle: &str) -> bool {
    let has = |text: &str| text.to_lowercase().contains(needle);
    has(&track.title)
        || track.artist.as_deref().is_some_and(has)
        || track.album.as_deref().is_some_and(has)
        || track.local_path().is_some_and(|path| {
            let relative = path.strip_prefix(root).unwrap_or(path);
            has(&relative.to_string_lossy())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackMetadata;
    use std::time::{Duration, Instant};

    fn track(rel: &str, title: Option<&str>, artist: Option<&str>, album: Option<&str>) -> Track {
        Track::local(
            format!("/music/{rel}").into(),
            TrackMetadata {
                title: title.map(Into::into),
                artist: artist.map(Into::into),
                album: album.map(Into::into),
                ..TrackMetadata::default()
            },
        )
    }

    fn tree(tracks: Vec<Track>) -> LibraryTree {
        let mut tree = LibraryTree::new("/music".into());
        tree.extend(tracks);
        tree
    }

    fn titles(results: &SearchResults) -> Vec<&str> {
        results.tracks.iter().map(|t| t.title.as_str()).collect()
    }

    #[test]
    fn matches_title_artist_album_and_relative_path_ignoring_case() {
        let tree = tree(vec![
            track("x/1.flac", Some("Blue in Green"), None, None),
            track("x/2.flac", Some("Two"), Some("BLUES Band"), None),
            track("x/3.flac", Some("Three"), None, Some("Kind of blue")),
            track("Blue Folder/4.flac", Some("Four"), None, None),
            track("x/5.flac", Some("Red"), Some("Green"), Some("Album")),
        ]);
        let results = search(&tree, "  bLuE ").unwrap();
        assert_eq!(results.query, "bLuE");
        assert_eq!(results.total, 4);
        // Browse order: directories sort case-insensitively.
        assert_eq!(titles(&results), ["Four", "Blue in Green", "Two", "Three"]);
        // The music root itself is not part of the matched path.
        assert_eq!(search(&tree, "music").unwrap().total, 0);
        assert_eq!(search(&tree, "x/5").unwrap().total, 1);
        // Non-ASCII case folding.
        let tree = tree_with_title("Ÿes ÉTÉ");
        assert_eq!(search(&tree, "été").unwrap().total, 1);
        assert_eq!(search(&tree, "ÿ").unwrap().total, 1);
        // An ASCII query still matches through Unicode lowercasing (the
        // Kelvin sign lowercases to `k`).
        let tree = tree_with_title("\u{212A}ilo");
        assert_eq!(search(&tree, "kilo").unwrap().total, 1);
    }

    fn tree_with_title(title: &str) -> LibraryTree {
        tree(vec![track("a.flac", Some(title), None, None)])
    }

    #[test]
    fn blank_queries_do_not_search() {
        let tree = tree_with_title("x");
        assert_eq!(search(&tree, ""), None);
        assert_eq!(search(&tree, " \t "), None);
        let none = search(&tree, "nothing").unwrap();
        assert!(none.tracks.is_empty());
        assert_eq!(none.total, 0);
    }

    #[test]
    fn results_are_capped_but_counted() {
        let tracks = (0..MAX_RESULTS + 234)
            .map(|i| track(&format!("d/{i:05}.flac"), None, None, None))
            .collect();
        let results = search(&tree(tracks), "d/").unwrap();
        assert_eq!(results.total, MAX_RESULTS + 234);
        assert_eq!(results.tracks.len(), MAX_RESULTS);
        assert_eq!(results.tracks[0].title, "00000");
    }

    /// 50,000 synthetic tracks with realistic tags. Prints the time per
    /// search (run with `--release --nocapture` to see the real cost); the
    /// assertion only guards against something pathological in debug
    /// builds.
    #[test]
    fn fifty_thousand_tracks_search_quickly() {
        let mut tracks = Vec::with_capacity(50_000);
        for i in 0..50_000 {
            let artist = format!("Artist Number {:04}", i / 100);
            let album = format!("Album Title Of Some Length {:04}", i / 10);
            tracks.push(track(
                &format!("{artist}/{album}/{:02} Track Name {i}.flac", i % 10),
                Some(&format!("Track Name {i} (Remastered Édition)")),
                Some(&artist),
                Some(&album),
            ));
        }
        let tree = tree(tracks);
        assert_eq!(tree.len(), 50_000);
        // A query that matches nothing checks every field of every track,
        // the worst case.
        let mut worst = Duration::ZERO;
        for query in ["zzz no match", "track name 4999", "ÉDITION"] {
            let start = Instant::now();
            let results = search(&tree, query).unwrap();
            let elapsed = start.elapsed();
            worst = worst.max(elapsed);
            println!("search {query:?}: {} matches in {elapsed:?}", results.total);
        }
        assert_eq!(search(&tree, "track name 49999").unwrap().total, 1);
        assert_eq!(search(&tree, "édition").unwrap().total, 50_000);
        assert!(worst < Duration::from_secs(2), "{worst:?}");
    }
}
