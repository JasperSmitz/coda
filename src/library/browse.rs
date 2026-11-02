//! Local browse navigation: a stack of views rooted at a static Home
//! (Playlists, Folders, Artists, Albums), over a [`Library`]. Plain
//! synchronous state with no I/O and no terminal types; separate from the
//! Navidrome browser, whose views are fetched.
//!
//! The library grows while the scan runs. Ids (tracks, artists, albums,
//! playlists) and folder paths stay valid as it grows, so views hold them
//! directly; the selection is kept on its item by key
//! ([`LocalBrowser::selected_key`] before a change,
//! [`LocalBrowser::reselect`] after it).

use super::tags::{Album, AlbumId, AlbumOrder, Artist, ArtistId};
use super::tree::{DirNode, TrackId};
use super::{Library, LocalPlaylist, PlaylistId};
use crate::model::Track;

/// An entry of the local Home view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeEntry {
    Playlists,
    Folders,
    Artists,
    Albums,
}

impl HomeEntry {
    pub fn label(self) -> &'static str {
        match self {
            Self::Playlists => "Playlists",
            Self::Folders => "Folders",
            Self::Artists => "Artists",
            Self::Albums => "Albums",
        }
    }
}

/// The Home view's rows, in order.
pub const HOME_ENTRIES: [HomeEntry; 4] = [
    HomeEntry::Playlists,
    HomeEntry::Folders,
    HomeEntry::Artists,
    HomeEntry::Albums,
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
}

/// What Enter or `s` plays: the tracks of the selection's context, in
/// order, and where to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayContext {
    /// The album, playlist, artist or folder name, or the track's name
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

    /// The album order Home → Albums opens (the last one chosen).
    pub fn album_order(&self) -> AlbumOrder {
        self.album_order
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
    /// entry, folder, artist, album or playlist. Returns `false` (and
    /// does nothing) on a track or in an empty view.
    pub fn open_selected(&mut self, library: &Library) -> bool {
        let Some(row) = self.selected_row(library) else {
            return false;
        };
        let view = match row {
            Row::Track(..) => return false,
            Row::Home(HomeEntry::Playlists) => View::Playlists,
            Row::Home(HomeEntry::Folders) => View::Folder(Vec::new()),
            Row::Home(HomeEntry::Artists) => View::Artists,
            Row::Home(HomeEntry::Albums) => View::Albums(self.album_order),
            Row::Dir(dir) => {
                let mut path = self.folder_path().to_vec();
                path.push(dir.name().to_owned());
                View::Folder(path)
            }
            Row::Artist(artist) => View::Artist(artist.id()),
            Row::Album(album, _) => View::Album(album.id()),
            Row::Playlist(playlist) => View::Playlist(playlist.id),
        };
        self.stack.push(Entry { view, selected: 0 });
        true
    }

    /// What Enter (`shuffle == false`) or `s` (`true`) on the selected
    /// row plays. A track plays in the list the view shows (an album, a
    /// playlist with its duplicates, the tracks directly in a folder),
    /// from that track; an album or playlist plays whole. Only `s` plays
    /// an artist (every album, A–Z) or a folder (every track below it, as
    /// `a` adds them); Enter opens those. `None` for Home entries, for
    /// Enter on an artist or folder, and in an empty view.
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
            Row::Artist(_) | Row::Dir(_) if !shuffle => return None,
            Row::Artist(artist) => whole(artist.name(), self.selected_tracks(library)?),
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
    /// album order; an album; a playlist with its duplicates. `None` for
    /// Home entries and empty views.
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
        }
        Some(ids)
    }

    /// The selected row's name, for status messages: a track's display
    /// name, or a folder, artist, album or playlist name.
    pub fn selected_label(&self, library: &Library) -> Option<String> {
        Some(match self.selected_row(library)? {
            Row::Home(entry) => entry.label().to_owned(),
            Row::Dir(dir) => dir.name().to_owned(),
            Row::Track(_, track) => track.display_name(),
            Row::Artist(artist) => artist.name().to_owned(),
            Row::Album(album, _) => album.name().to_owned(),
            Row::Playlist(playlist) => playlist.name.clone(),
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
        _ => None,
    }
}
