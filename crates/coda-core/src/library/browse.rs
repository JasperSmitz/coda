//! Local browse navigation: a stack of views rooted at a static Home
//! (Shuffle all, Playlists, Folders, Artists, Albums, Genres), over a
//! [`Library`]. Plain
//! synchronous state with no I/O and no terminal types; separate from the
//! Navidrome browser, whose views are fetched.
//!
//! The library grows while the scan runs. Ids (tracks, artists, albums,
//! genres, playlists) and folder paths stay valid as it grows, so views hold them
//! directly; the selection is kept on its item by key
//! ([`LocalBrowser::selected_key`] before a change,
//! [`LocalBrowser::reselect`] after it).

use super::tags::{Album, AlbumId, AlbumOrder, Artist, ArtistId, Genre, GenreId};
use super::tree::{DirNode, TrackId};
use super::{Library, LocalPlaylist, PlaylistId};
use crate::model::Track;

/// An entry of the local Home view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeEntry {
    /// Plays the whole library shuffled; handled by the app (it needs the
    /// random generator), never opened.
    ShuffleAll,
    Playlists,
    Folders,
    Artists,
    Albums,
    Genres,
}

impl HomeEntry {
    pub fn label(self) -> &'static str {
        match self {
            Self::ShuffleAll => "Shuffle all",
            Self::Playlists => "Playlists",
            Self::Folders => "Folders",
            Self::Artists => "Artists",
            Self::Albums => "Albums",
            Self::Genres => "Genres",
        }
    }
}

/// The Home view's rows, in order.
pub const HOME_ENTRIES: [HomeEntry; 6] = [
    HomeEntry::ShuffleAll,
    HomeEntry::Playlists,
    HomeEntry::Folders,
    HomeEntry::Artists,
    HomeEntry::Albums,
    HomeEntry::Genres,
];

/// What a local view shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Home,
    /// A folder by its path from the music root (empty: the root).
    Folder(Vec<String>),
    Artists,
    /// An artist's albums.
    Artist(ArtistId),
    /// Every album.
    Albums(AlbumOrder),
    /// An album's tracks.
    Album(AlbumId),
    Playlists,
    /// A playlist's matched tracks in file order.
    Playlist(PlaylistId),
    /// Every genre, A–Z.
    Genres,
    /// A genre's albums, by artist then album.
    Genre(GenreId),
}

/// One row of a local view.
#[derive(Debug, Clone, Copy)]
pub enum Row<'a> {
    Home(HomeEntry),
    Dir(&'a DirNode),
    Track(TrackId, &'a Track),
    Artist(&'a Artist),
    /// An album and its artist.
    Album(&'a Album, &'a Artist),
    Playlist(&'a LocalPlaylist),
    Genre(&'a Genre),
}

/// Identity of a row, to keep the selection on its item while rows are
/// inserted around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKey {
    Home(HomeEntry),
    Dir(String),
    Track(TrackId),
    Artist(ArtistId),
    Album(AlbumId),
    Playlist(PlaylistId),
    Genre(GenreId),
}

/// What Enter or `s` plays: the tracks of the selection's context, in
/// order, and where to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayContext {
    /// The album, playlist, artist, genre or folder name, or the track's name
    /// when a track was selected (untrusted: from tags and file names).
    pub label: String,
    pub tracks: Vec<TrackId>,
    /// The selected track's index in `tracks`; `None` when a whole album,
    /// playlist, artist or folder was selected.
    pub start: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    view: View,
    selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalBrowser {
    /// Never empty; `stack[0]` is Home.
    stack: Vec<Entry>,
    /// The album order last chosen; Home → Albums opens it.
    album_order: AlbumOrder,
}

impl Default for LocalBrowser {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalBrowser {
    pub fn new() -> Self {
        Self {
            stack: vec![Entry {
                view: View::Home,
                selected: 0,
            }],
            album_order: AlbumOrder::default(),
        }
    }

    fn top(&self) -> &Entry {
        self.stack.last().expect("the stack always holds Home")
    }

    fn top_mut(&mut self) -> &mut Entry {
        self.stack.last_mut().expect("the stack always holds Home")
    }

    /// The shown view.
    pub fn current(&self) -> &View {
        &self.top().view
    }

    /// The views from Home to the shown one.
    pub fn views(&self) -> impl Iterator<Item = &View> {
        self.stack.iter().map(|e| &e.view)
    }

    /// Selected row of the shown view; in range whenever it has rows.
    pub fn selected(&self) -> usize {
        self.top().selected
    }

    /// The folder path when a folder is shown, else empty.
    pub fn folder_path(&self) -> &[String] {
        match self.current() {
            View::Folder(path) => path,
            _ => &[],
        }
    }

    /// Number of rows in the shown view.
    pub fn len(&self, library: &Library) -> usize {
        view_len(library, self.current())
    }

    pub fn is_empty(&self, library: &Library) -> bool {
        self.len(library) == 0
    }

    /// At most `len` rows starting at `start`, without building the rest.
    pub fn rows<'a>(&self, library: &'a Library, start: usize, len: usize) -> Vec<Row<'a>> {
        view_rows(library, self.current(), start, len)
    }

    pub fn selected_row<'a>(&self, library: &'a Library) -> Option<Row<'a>> {
        self.rows(library, self.selected(), 1).pop()
    }

    /// The key of the selected row, before the library changes.
    pub fn selected_key(&self, library: &Library) -> Option<RowKey> {
        self.selected_row(library).map(row_key)
    }

    /// After the library grew: puts the selection back on the row with
    /// `key` (if it is still shown) and keeps it in range. Home and
    /// playlists never change, so their selection is left alone (a
    /// playlist can hold a track twice).
    pub fn reselect(&mut self, library: &Library, key: Option<RowKey>) {
        let view = self.current().clone();
        if !matches!(view, View::Home | View::Playlist(_))
            && let Some(at) = key.and_then(|k| position(library, &view, &k))
        {
            self.top_mut().selected = at;
        }
        self.clamp(library);
    }

    /// Keeps the selection in range.
    pub fn clamp(&mut self, library: &Library) {
        let len = self.len(library);
        let top = self.top_mut();
        top.selected = top.selected.min(len.saturating_sub(1));
    }

    pub fn move_selection(&mut self, library: &Library, delta: isize) {
        let len = self.len(library);
        let top = self.top_mut();
        top.selected = top
            .selected
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
    }

    /// `l` (and Enter on what it cannot play): opens the selected Home
    /// entry, folder, artist, album, playlist or genre. Returns `false` (and
    /// does nothing) on a track, on "Shuffle all" or in an empty view.
    pub fn open_selected(&mut self, library: &Library) -> bool {
        let Some(row) = self.selected_row(library) else {
            return false;
        };
        let view = match row {
            Row::Track(..) | Row::Home(HomeEntry::ShuffleAll) => return false,
            Row::Home(HomeEntry::Playlists) => View::Playlists,
            Row::Home(HomeEntry::Folders) => View::Folder(Vec::new()),
            Row::Home(HomeEntry::Artists) => View::Artists,
            Row::Home(HomeEntry::Albums) => View::Albums(self.album_order),
            Row::Home(HomeEntry::Genres) => View::Genres,
            Row::Dir(dir) => {
                let mut path = self.folder_path().to_vec();
                path.push(dir.name().to_owned());
                View::Folder(path)
            }
            Row::Artist(artist) => View::Artist(artist.id()),
            Row::Album(album, _) => View::Album(album.id()),
            Row::Playlist(playlist) => View::Playlist(playlist.id),
            Row::Genre(genre) => View::Genre(genre.id()),
        };
        self.stack.push(Entry { view, selected: 0 });
        true
    }

    /// What Enter (`shuffle == false`) or `s` (`true`) on the selected
    /// row plays. A track plays in the list the view shows (an album, a
    /// playlist with its duplicates, the tracks directly in a folder),
    /// from that track; an album or playlist plays whole. Only `s` plays
    /// an artist (every album, A–Z), a genre (every track of it, as `a`
    /// adds them) or a folder (every track below it, as `a` adds them);
    /// Enter opens those. `None` for Home entries, for Enter on an artist,
    /// genre or folder, and in an empty view.
    pub fn play_context(&self, library: &Library, shuffle: bool) -> Option<PlayContext> {
        let row = self.selected_row(library)?;
        let whole = |label: &str, tracks: Vec<TrackId>| PlayContext {
            label: label.to_owned(),
            tracks,
            start: None,
        };
        Some(match row {
            Row::Home(_) => return None,
            Row::Track(_, track) => {
                let (tracks, start) = match self.current() {
                    View::Folder(path) => {
                        let dir = folder(library, path);
                        (dir.tracks().to_vec(), self.selected() - dir.dirs().len())
                    }
                    View::Album(id) => {
                        (library.tags().album(*id).tracks().to_vec(), self.selected())
                    }
                    View::Playlist(id) => (library.playlist(*id).tracks.clone(), self.selected()),
                    // Tracks are only shown in the views above.
                    _ => return None,
                };
                PlayContext {
                    label: track.display_name(),
                    tracks,
                    start: Some(start),
                }
            }
            Row::Album(album, _) => whole(album.name(), album.tracks().to_vec()),
            Row::Playlist(playlist) => whole(&playlist.name, playlist.tracks.clone()),
            Row::Artist(_) | Row::Dir(_) | Row::Genre(_) if !shuffle => return None,
            Row::Artist(artist) => whole(artist.name(), self.selected_tracks(library)?),
            Row::Genre(genre) => whole(genre.name(), self.selected_tracks(library)?),
            Row::Dir(dir) => whole(dir.name(), self.selected_tracks(library)?),
        })
    }

    /// Back to the previous view, selecting the row that opened the one
    /// left (rows may have moved since). Home stays.
    pub fn go_up(&mut self, library: &Library) {
        if self.stack.len() == 1 {
            return;
        }
        let left = self.stack.pop().map(|e| e.view);
        if let Some(key) = left.as_ref().and_then(opener_key) {
            let view = self.current().clone();
            if let Some(at) = position(library, &view, &key) {
                self.top_mut().selected = at;
            }
        }
        self.clamp(library);
    }

    /// `o` in the Albums list: shows the next order, keeping the selected
    /// album selected. Returns `false` (and does nothing) elsewhere.
    pub fn cycle_album_order(&mut self, library: &Library) -> bool {
        let View::Albums(order) = *self.current() else {
            return false;
        };
        let key = self.selected_key(library);
        self.album_order = order.next();
        self.top_mut().view = View::Albums(self.album_order);
        self.top_mut().selected = 0;
        self.reselect(library, key);
        true
    }

    /// The tracks `a`/`A` adds for the selected row, in order: a track; a
    /// folder's tracks in browse order; an artist's albums A–Z, each in
    /// album order; an album; a playlist with its duplicates; a genre's
    /// tracks ([`super::tags::TagIndex::genre_tracks`]). `None` for Home
    /// entries and empty views.
    pub fn selected_tracks(&self, library: &Library) -> Option<Vec<TrackId>> {
        let mut ids = Vec::new();
        match self.selected_row(library)? {
            Row::Home(_) => return None,
            Row::Track(id, _) => ids.push(id),
            Row::Dir(dir) => library.tree().for_each_id(dir, |id| ids.push(id)),
            Row::Artist(artist) => {
                for album in artist.albums() {
                    ids.extend_from_slice(library.tags().album(*album).tracks());
                }
            }
            Row::Album(album, _) => ids.extend_from_slice(album.tracks()),
            Row::Playlist(playlist) => ids.extend_from_slice(&playlist.tracks),
            Row::Genre(genre) => ids = library.tags().genre_tracks(genre.id()),
        }
        Some(ids)
    }

    /// The selected row's name, for status messages: a track's display
    /// name, or a folder, artist, album, playlist or genre name.
    pub fn selected_label(&self, library: &Library) -> Option<String> {
        Some(match self.selected_row(library)? {
            Row::Home(entry) => entry.label().to_owned(),
            Row::Dir(dir) => dir.name().to_owned(),
            Row::Track(_, track) => track.display_name(),
            Row::Artist(artist) => artist.name().to_owned(),
            Row::Album(album, _) => album.name().to_owned(),
            Row::Playlist(playlist) => playlist.name.clone(),
            Row::Genre(genre) => genre.name().to_owned(),
        })
    }

    /// Title of each view below Home, e.g. `["Artists", "Name", "Album"]`
    /// or `["Folders", "a", "b"]`.
    pub fn path_titles(&self, library: &Library) -> Vec<String> {
        self.views()
            .filter_map(|view| view_title(library, view))
            .collect()
    }
}

/// A view's title in the path; `None` for Home.
pub fn view_title(library: &Library, view: &View) -> Option<String> {
    Some(match view {
        View::Home => return None,
        View::Folder(path) => match path.last() {
            Some(name) => name.clone(),
            None => HomeEntry::Folders.label().to_owned(),
        },
        View::Artists => HomeEntry::Artists.label().to_owned(),
        View::Artist(id) => library.tags().artist(*id).name().to_owned(),
        View::Albums(order) => format!("Albums ({})", order.label()),
        View::Album(id) => library.tags().album(*id).name().to_owned(),
        View::Playlists => HomeEntry::Playlists.label().to_owned(),
        View::Playlist(id) => {
            let playlist = library.playlist(*id);
            if playlist.cut {
                format!("{} (cut: file too long)", playlist.name)
            } else {
                playlist.name.clone()
            }
        }
        View::Genres => HomeEntry::Genres.label().to_owned(),
        View::Genre(id) => library.tags().genre(*id).name().to_owned(),
    })
}

/// The key of the row in the parent view that opens `view`.
fn opener_key(view: &View) -> Option<RowKey> {
    Some(match view {
        View::Home => return None,
        View::Folder(path) => match path.last() {
            Some(name) => RowKey::Dir(name.clone()),
            None => RowKey::Home(HomeEntry::Folders),
        },
        View::Artists => RowKey::Home(HomeEntry::Artists),
        View::Albums(_) => RowKey::Home(HomeEntry::Albums),
        View::Playlists => RowKey::Home(HomeEntry::Playlists),
        View::Artist(id) => RowKey::Artist(*id),
        View::Album(id) => RowKey::Album(*id),
        View::Playlist(id) => RowKey::Playlist(*id),
        View::Genres => RowKey::Home(HomeEntry::Genres),
        View::Genre(id) => RowKey::Genre(*id),
    })
}

fn row_key(row: Row<'_>) -> RowKey {
    match row {
        Row::Home(entry) => RowKey::Home(entry),
        Row::Dir(dir) => RowKey::Dir(dir.name().to_owned()),
        Row::Track(id, _) => RowKey::Track(id),
        Row::Artist(artist) => RowKey::Artist(artist.id()),
        Row::Album(album, _) => RowKey::Album(album.id()),
        Row::Playlist(playlist) => RowKey::Playlist(playlist.id),
        Row::Genre(genre) => RowKey::Genre(genre.id()),
    }
}

fn folder<'a>(library: &'a Library, path: &[String]) -> &'a DirNode {
    // The tree only grows, so a path that was shown always resolves.
    library
        .tree()
        .dir(path)
        .unwrap_or_else(|| library.tree().root())
}

fn view_len(library: &Library, view: &View) -> usize {
    let tags = library.tags();
    match view {
        View::Home => HOME_ENTRIES.len(),
        View::Folder(path) => {
            let dir = folder(library, path);
            dir.dirs().len() + dir.tracks().len()
        }
        View::Artists => tags.artists().len(),
        View::Artist(id) => tags.artist(*id).albums().len(),
        View::Albums(order) => tags.albums(*order).len(),
        View::Album(id) => tags.album(*id).tracks().len(),
        View::Playlists => library.playlists().len(),
        View::Playlist(id) => library.playlist(*id).tracks.len(),
        View::Genres => tags.genres().len(),
        View::Genre(id) => tags.genre(*id).albums().len(),
    }
}

fn window<T>(items: &[T], start: usize, len: usize) -> &[T] {
    let start = start.min(items.len());
    &items[start..items.len().min(start.saturating_add(len))]
}

fn view_rows<'a>(library: &'a Library, view: &View, start: usize, len: usize) -> Vec<Row<'a>> {
    let tags = library.tags();
    let track = |id: &TrackId| Row::Track(*id, library.track(*id));
    let album = |id: &AlbumId| {
        let album = tags.album(*id);
        Row::Album(album, tags.artist(album.artist()))
    };
    match view {
        View::Home => window(&HOME_ENTRIES, start, len)
            .iter()
            .map(|e| Row::Home(*e))
            .collect(),
        View::Folder(path) => {
            let dir = folder(library, path);
            dir.dirs()
                .iter()
                .map(Row::Dir)
                .chain(dir.tracks().iter().map(track))
                .skip(start)
                .take(len)
                .collect()
        }
        View::Artists => window(tags.artists(), start, len)
            .iter()
            .map(|a| Row::Artist(tags.artist(*a)))
            .collect(),
        View::Artist(id) => window(tags.artist(*id).albums(), start, len)
            .iter()
            .map(album)
            .collect(),
        View::Albums(order) => window(tags.albums(*order), start, len)
            .iter()
            .map(album)
            .collect(),
        View::Album(id) => window(tags.album(*id).tracks(), start, len)
            .iter()
            .map(track)
            .collect(),
        View::Playlists => window(library.playlists(), start, len)
            .iter()
            .map(|p| Row::Playlist(library.playlist(*p)))
            .collect(),
        View::Playlist(id) => window(&library.playlist(*id).tracks, start, len)
            .iter()
            .map(track)
            .collect(),
        View::Genres => window(tags.genres(), start, len)
            .iter()
            .map(|g| Row::Genre(tags.genre(*g)))
            .collect(),
        View::Genre(id) => window(tags.genre(*id).albums(), start, len)
            .iter()
            .map(album)
            .collect(),
    }
}

/// Where the row with `key` is in `view`. Linear, so only used when the
/// library changes or a view is left.
fn position(library: &Library, view: &View, key: &RowKey) -> Option<usize> {
    let tags = library.tags();
    match (view, key) {
        (View::Home, RowKey::Home(entry)) => HOME_ENTRIES.iter().position(|e| e == entry),
        (View::Folder(path), RowKey::Dir(name)) => folder(library, path)
            .dirs()
            .iter()
            .position(|d| d.name() == name),
        (View::Folder(path), RowKey::Track(id)) => {
            let dir = folder(library, path);
            dir.tracks()
                .iter()
                .position(|t| t == id)
                .map(|i| i + dir.dirs().len())
        }
        (View::Artists, RowKey::Artist(id)) => tags.artists().iter().position(|a| a == id),
        (View::Artist(artist), RowKey::Album(id)) => {
            tags.artist(*artist).albums().iter().position(|a| a == id)
        }
        (View::Albums(order), RowKey::Album(id)) => {
            tags.albums(*order).iter().position(|a| a == id)
        }
        (View::Album(album), RowKey::Track(id)) => {
            tags.album(*album).tracks().iter().position(|t| t == id)
        }
        (View::Playlists, RowKey::Playlist(id)) => library.playlists().iter().position(|p| p == id),
        (View::Playlist(playlist), RowKey::Track(id)) => library
            .playlist(*playlist)
            .tracks
            .iter()
            .position(|t| t == id),
        (View::Genres, RowKey::Genre(id)) => tags.genres().iter().position(|g| g == id),
        (View::Genre(genre), RowKey::Album(id)) => {
            tags.genre(*genre).albums().iter().position(|a| a == id)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackMetadata;

    fn track(path: &str, artist: &str, album: &str, no: u32, genres: &[&str]) -> Track {
        Track::local(
            format!("/m/{path}").into(),
            TrackMetadata {
                artist: Some(artist.into()),
                album: Some(album.into()),
                track_no: Some(no),
                genres: genres.iter().map(|g| (*g).to_owned()).collect(),
                ..TrackMetadata::default()
            },
        )
    }

    fn library(tracks: Vec<Track>) -> Library {
        let mut library = Library::new("/m".into());
        library.extend(tracks);
        library
    }

    /// Home → Genres, opened as a user does: down to the last Home entry.
    fn genres_view(library: &Library) -> LocalBrowser {
        let mut browser = LocalBrowser::new();
        let at = HOME_ENTRIES
            .iter()
            .position(|e| *e == HomeEntry::Genres)
            .unwrap();
        browser.move_selection(library, at as isize);
        assert_eq!(browser.selected_label(library).as_deref(), Some("Genres"));
        assert!(browser.open_selected(library));
        assert_eq!(*browser.current(), View::Genres);
        browser
    }

    #[test]
    fn home_lists_genres_after_albums() {
        let library = library(Vec::new());
        let browser = LocalBrowser::new();
        let labels: Vec<&str> = browser
            .rows(&library, 0, 10)
            .into_iter()
            .map(|row| match row {
                Row::Home(entry) => entry.label(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            labels,
            [
                "Shuffle all",
                "Playlists",
                "Folders",
                "Artists",
                "Albums",
                "Genres"
            ]
        );
        // The entry itself only opens: nothing to play or add.
        let mut home = LocalBrowser::new();
        home.move_selection(&library, 5);
        assert_eq!(
            home.selected_key(&library),
            Some(RowKey::Home(HomeEntry::Genres))
        );
        assert_eq!(home.play_context(&library, false), None);
        assert_eq!(home.play_context(&library, true), None);
        assert_eq!(home.selected_tracks(&library), None);
        let mut genres = genres_view(&library);
        assert_eq!(genres.path_titles(&library), ["Genres"]);
        // `h` comes back to the Genres entry.
        genres.go_up(&library);
        assert_eq!(
            genres.selected_key(&library),
            Some(RowKey::Home(HomeEntry::Genres))
        );
    }

    fn titles(library: &Library, ids: &[TrackId]) -> Vec<String> {
        ids.iter()
            .map(|t| library.track(*t).title.clone())
            .collect()
    }

    #[test]
    fn genres_list_a_to_z_and_open_on_their_albums() {
        let library = library(vec![
            track("z/1.flac", "Zed", "Zz", 1, &["Rock"]),
            track("a/1.flac", "Abe", "Aa", 1, &["rock", "Jazz"]),
            track("a/2.flac", "Abe", "Aa", 2, &["Jazz"]),
        ]);
        let mut browser = genres_view(&library);
        assert_eq!(browser.path_titles(&library), ["Genres"]);
        assert_eq!(browser.len(&library), 2);
        let names: Vec<String> = browser
            .rows(&library, 0, 10)
            .into_iter()
            .map(|row| match row {
                Row::Genre(genre) => genre.name().to_owned(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(names, ["Jazz", "Rock"]);
        browser.move_selection(&library, 1);
        assert_eq!(browser.selected_label(&library).as_deref(), Some("Rock"));
        // Enter opens a genre; `s` plays all of it.
        assert_eq!(browser.play_context(&library, false), None);
        let shuffled = browser.play_context(&library, true).unwrap();
        assert_eq!(shuffled.label, "Rock");
        assert_eq!(titles(&library, &shuffled.tracks), ["1", "1"]);
        assert_eq!(shuffled.start, None);
        assert_eq!(
            browser.selected_tracks(&library),
            Some(shuffled.tracks.clone())
        );
        assert!(browser.open_selected(&library));
        let View::Genre(rock) = *browser.current() else {
            panic!("not a genre view");
        };
        assert_eq!(browser.path_titles(&library), ["Genres", "Rock"]);
        let albums: Vec<&str> = browser
            .rows(&library, 0, 10)
            .into_iter()
            .map(|row| match row {
                Row::Album(album, _) => album.name(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(albums, ["Aa", "Zz"]);
        // An album row plays the whole album, not only its Rock tracks.
        let album = browser.play_context(&library, false).unwrap();
        assert_eq!(album.label, "Aa");
        assert_eq!(album.tracks.len(), 2);
        // Back on the genre that was open.
        browser.go_up(&library);
        assert_eq!(browser.selected_key(&library), Some(RowKey::Genre(rock)));
    }

    #[test]
    fn the_genre_selection_stays_on_its_genre_while_the_scan_adds_more() {
        let mut library = library(vec![track("b/1.flac", "B", "B", 1, &["Blues"])]);
        let mut browser = genres_view(&library);
        let key = browser.selected_key(&library);
        library.extend([track("a/1.flac", "A", "A", 1, &["Ambient"])]);
        browser.reselect(&library, key);
        assert_eq!(browser.selected(), 1);
        assert_eq!(browser.selected_label(&library).as_deref(), Some("Blues"));
    }

    #[test]
    fn an_empty_library_has_no_genres() {
        let library = library(Vec::new());
        let browser = genres_view(&library);
        assert!(browser.is_empty(&library));
        assert_eq!(browser.play_context(&library, true), None);
    }
}
