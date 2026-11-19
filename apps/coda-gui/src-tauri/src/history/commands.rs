//! The play history's commands: Home's Where you left off and Moods for
//! today of one source, and an artist's most played songs. History comes
//! from the engine (a copy); resolving local keys against the library,
//! reading the mixes file and fetching from the server happen here, off
//! the engine task.

use std::collections::HashMap;
use std::time::Duration;

use coda_core::mix::{Ingredient, Mix, MixSource};
use coda_core::model::{Track, TrackMetadata};
use coda_core::navidrome::UNKNOWN_ARTIST;
use serde::Serialize;
use tauri::State;

use super::moods::{self, GenreSize, Listens};
use super::{
    ContextKind, HomeDto, HomeRows, LocalIndex, Mood, MoodAlbum, Scope, SourceKind, TOP_TRACKS,
    TrackCount, home_dto, local_album_key, local_artist_top_tracks, server_id, unix_now,
    valid_date,
};
use crate::browse::dto::{BrowseError, BrowseErrorKind};
use crate::browse::{BrowseHandle, LocalView, check_genre, check_id, local, track_dto};
use crate::cover::CoverService;
use crate::dto::TrackDto;
use crate::engine::{EngineGone, EngineHandle, MoodsRequest};
use crate::mixes::{IngredientDto, MixesFile};

type Result<T> = std::result::Result<T, BrowseError>;

fn gone(_: EngineGone) -> BrowseError {
    BrowseError::new(BrowseErrorKind::Failed, "Coda is quitting")
}

/// The saved mixes of `source` (Navidrome: the current server's) by id,
/// read only when `rows` holds a mix (blocking, off the engine).
async fn mixes_for(path: Option<std::path::PathBuf>, rows: &HomeRows) -> HashMap<String, Mix> {
    let Some(path) = path else {
        return HashMap::new();
    };
    if !rows
        .recent
        .iter()
        .any(|p| p.context.kind == ContextKind::Mix)
    {
        return HashMap::new();
    }
    tokio::task::spawn_blocking(move || {
        coda_core::mix::store::load(&path)
            .0
            .into_iter()
            .map(|mix| (mix.id.to_string(), mix))
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Resolves a source's rows (local ones against the current library; a
/// mix only if it is still saved, for the same source and server).
pub async fn resolve_home(
    browse: &BrowseHandle,
    source: SourceKind,
    rows: HomeRows,
    mixes: HashMap<String, Mix>,
) -> Result<HomeDto> {
    match source {
        SourceKind::Local => {
            let mixes: HashMap<String, Mix> = mixes
                .into_iter()
                .filter(|(_, m)| m.source == MixSource::Local)
                .collect();
            browse
                .local(move |view, covers| Ok(home_dto(rows, Some(&view), &mixes, covers)))
                .await
        }
        SourceKind::Navidrome => {
            let (client, _) = browse.client()?;
            let server = server_id(&client);
            let mixes: HashMap<String, Mix> = mixes
                .into_iter()
                .filter(|(_, m)| matches!(&m.source, MixSource::Navidrome { server: s } if *s == server))
                .collect();
            Ok(home_dto(rows, None, &mixes, browse.covers()))
        }
    }
}

/// Where you left off: the source's recent contexts, with where each
/// stopped.
#[tauri::command]
pub async fn home_history(
    engine: State<'_, EngineHandle>,
    browse: State<'_, BrowseHandle>,
    mixes: State<'_, MixesFile>,
    source: SourceKind,
) -> Result<HomeDto> {
    let rows = engine.home_rows(source).await.map_err(gone)?;
    let mixes = mixes_for(mixes.0.clone(), &rows).await;
    resolve_home(&browse, source, rows, mixes).await
}

// ----- Moods for today --------------------------------------------------

/// One album of a mood card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoodAlbumDto {
    /// The id to open it by (a local id of this library generation, or
    /// Navidrome's).
    pub id: String,
    pub name: String,
    pub artist: Option<String>,
    pub artist_id: Option<String>,
    pub cover_url: Option<String>,
    pub duration_ms: Option<u64>,
    /// The last listen this app counted (unix seconds); `None`: never.
    pub last_played: Option<u64>,
    /// The album as a quick mix's ingredient.
    pub ingredient: IngredientDto,
}

/// One mood card: a genre and its albums.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoodDto {
    pub genre: String,
    /// The genre's albums in the library (on the server), when known.
    pub album_count: Option<u32>,
    pub albums: Vec<MoodAlbumDto>,
}

/// The day's moods: today's three, then the other three.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoodsDto {
    pub today: Vec<MoodDto>,
    pub other: Vec<MoodDto>,
}

/// Genres of a Navidrome server whose albums are fetched for one day's
/// moods, at most (in rank order; fewer once every mood is full).
pub const MAX_GENRE_FETCHES: usize = 12;
/// Albums fetched per Navidrome genre (one page).
const GENRE_ALBUMS: usize = 500;

fn split(moods: Vec<MoodDto>) -> MoodsDto {
    let mut today = moods;
    let other = today
        .split_off(moods::MOODS_SHOWN.min(today.len()))
        .into_iter()
        .take(moods::MOODS_SHOWN)
        .collect();
    MoodsDto { today, other }
}

/// Moods for today of `date` (the UI's local calendar day, `YYYY-MM-DD`;
/// else `notFound`): picked at the first ask of the day (`history::moods`)
/// and cached in the history; the albums' facts are read fresh.
#[tauri::command]
pub async fn home_moods(
    engine: State<'_, EngineHandle>,
    browse: State<'_, BrowseHandle>,
    source: SourceKind,
    date: String,
) -> Result<MoodsDto> {
    day_moods(&engine, &browse, source, date).await
}

/// A lock per (server, date) of Navidrome moods being picked.
type PickLocks = HashMap<(String, String), std::sync::Arc<tokio::sync::Mutex<()>>>;
/// Navidrome moods being picked, per server and date: a second ask of the
/// same day (another Home mounted meanwhile) waits for the first and then
/// finds them cached, instead of fetching the genres again.
static PICKING: std::sync::LazyLock<std::sync::Mutex<PickLocks>> =
    std::sync::LazyLock::new(Default::default);

/// The lock for picking `server`'s moods of `date` (the same one for
/// every ask of that day), and forgetting it once no one holds it.
struct Picking {
    key: (String, String),
    lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl Picking {
    fn of(server: &str, date: &str) -> Self {
        let key = (server.to_owned(), date.to_owned());
        let mut map = PICKING.lock().unwrap_or_else(|e| e.into_inner());
        let lock = map.entry(key.clone()).or_default().clone();
        Self { key, lock }
    }
}

impl Drop for Picking {
    fn drop(&mut self) {
        let mut map = PICKING.lock().unwrap_or_else(|e| e.into_inner());
        // Only the map's and this one's: no one else waits.
        if std::sync::Arc::strong_count(&self.lock) <= 2 {
            map.remove(&self.key);
        }
    }
}

pub async fn day_moods(
    engine: &EngineHandle,
    browse: &BrowseHandle,
    source: SourceKind,
    date: String,
) -> Result<MoodsDto> {
    if !valid_date(&date) {
        return Err(BrowseError::not_found("Date"));
    }
    // Navidrome: one pick per server and day at a time (the moods are
    // read from the history after the lock, so a waiter finds them).
    let picking = match source {
        SourceKind::Navidrome => {
            let (client, _) = browse.client()?;
            Some(Picking::of(&server_id(&client), &date))
        }
        SourceKind::Local => None,
    };
    let _guard = match &picking {
        Some(picking) => Some(picking.lock.lock().await),
        None => None,
    };
    let request = engine
        .moods_request(source, date.clone())
        .await
        .map_err(gone)?
        .ok_or_else(BrowseError::no_server)?;
    let MoodsRequest {
        scope,
        cached,
        listens,
        heard,
    } = request;
    let now = unix_now();
    match scope {
        Scope::Local => {
            let moods = match cached {
                Some(moods) => moods,
                None => {
                    let picks = {
                        let (listens, date) = (listens.clone(), date.clone());
                        browse
                            .local(move |view, _| Ok(local_moods(&view, &listens, now, &date)))
                            .await?
                    };
                    engine
                        .set_moods(Scope::Local, date, picks)
                        .await
                        .map_err(gone)?
                        .unwrap_or_default()
                }
            };
            browse
                .local(move |view, covers| {
                    Ok(split(local_mood_dtos(&view, &moods, &listens, covers)))
                })
                .await
        }
        Scope::Navidrome(server) => {
            let moods = match cached {
                Some(moods) => moods,
                None => {
                    let picks =
                        navidrome_moods(browse, &server, &heard, &listens, now, &date).await?;
                    engine
                        .set_moods(Scope::Navidrome(server), date, picks)
                        .await
                        .map_err(gone)?
                        .ok_or_else(BrowseError::stale)?
                }
            };
            Ok(split(navidrome_mood_dtos(
                &moods,
                &listens,
                browse.covers(),
            )))
        }
    }
}

/// The day's local moods, from the tag index.
pub fn local_moods(
    view: &LocalView<'_>,
    listens: &HashMap<String, Listens>,
    now: u64,
    date: &str,
) -> Vec<Mood> {
    let tags = view.library.tags();
    let albums = tags.albums(coda_core::library::tags::AlbumOrder::Name);
    let mut keys = vec![String::new(); albums.len()];
    for id in albums {
        keys[id.0 as usize] = local_album_key(tags, *id);
    }
    // Each album's genres, for the heard albums' ranking.
    let mut genres_of: HashMap<&str, Vec<String>> = HashMap::new();
    let mut sizes = Vec::new();
    for genre in tags.genres() {
        let genre = tags.genre(*genre);
        sizes.push(GenreSize {
            name: genre.name().to_owned(),
            albums: genre.albums().len(),
        });
        for album in genre.albums() {
            genres_of
                .entry(keys[album.0 as usize].as_str())
                .or_default()
                .push(genre.name().to_owned());
        }
    }
    let heard: Vec<(Vec<String>, Listens)> = listens
        .iter()
        .filter_map(|(key, l)| Some((genres_of.get(key.as_str())?.clone(), *l)))
        .collect();
    let ranked = moods::rank_genres(&sizes, &heard, now);
    let picks = moods::pick_moods(
        &ranked,
        |name| {
            tags.genre_by_name(name)
                .map(|g| {
                    tags.genre(g)
                        .albums()
                        .iter()
                        .map(|a| keys[a.0 as usize].clone())
                        .collect()
                })
                .unwrap_or_default()
        },
        listens,
        now,
        &moods::day_seed(date, None),
    );
    let index = LocalIndex::new(view);
    picks
        .into_iter()
        .map(|pick| Mood {
            genre: pick.genre,
            album_count: None,
            albums: pick
                .albums
                .into_iter()
                .map(|key| {
                    let album = index.album(&key).map(|id| tags.album(id));
                    MoodAlbum {
                        name: album.map(|a| a.name().to_owned()).unwrap_or_default(),
                        artist: album.map(|a| tags.artist(a.artist()).name().to_owned()),
                        id: key,
                        artist_id: None,
                        cover_art: None,
                        duration_ms: None,
                    }
                })
                .collect(),
        })
        .collect()
}

/// The cached local moods as cards (albums no longer in the library left
/// out, and a mood left with none).
pub fn local_mood_dtos(
    view: &LocalView<'_>,
    moods: &[Mood],
    listens: &HashMap<String, Listens>,
    covers: &CoverService,
) -> Vec<MoodDto> {
    let index = LocalIndex::new(view);
    let tags = view.library.tags();
    moods
        .iter()
        .filter_map(|mood| {
            let albums: Vec<MoodAlbumDto> = mood
                .albums
                .iter()
                .filter_map(|cached| {
                    let id = index.album(&cached.id)?;
                    let dto = local::album_dto(view, id, covers);
                    let album = tags.album(id);
                    let artist = tags.artist(album.artist()).name().to_owned();
                    Some(MoodAlbumDto {
                        id: dto.id,
                        name: dto.name,
                        artist_id: dto.artist_id,
                        cover_url: dto.cover_url,
                        duration_ms: dto.duration_ms,
                        last_played: listens.get(&cached.id).map(|l| l.last),
                        ingredient: IngredientDto::from(&Ingredient::Album {
                            artist: artist.clone(),
                            name: album.name().to_owned(),
                            id: None,
                        }),
                        artist: Some(artist),
                    })
                })
                .collect();
            let count = tags
                .genre_by_name(&mood.genre)
                .map(|g| u32::try_from(tags.genre(g).albums().len()).unwrap_or(u32::MAX));
            (!albums.is_empty()).then(|| MoodDto {
                genre: tags
                    .genre_by_name(&mood.genre)
                    .map_or_else(|| mood.genre.clone(), |g| tags.genre(g).name().to_owned()),
                album_count: count,
                albums,
            })
        })
        .collect()
}

/// The day's Navidrome moods: the server's genres (`getGenres`), ranked
/// with the heard albums' genres; the albums of the genres in rank order
/// fetched one at a time (`getAlbumList2 byGenre`, within the shared
/// fetch limit) until every mood is full or [`MAX_GENRE_FETCHES`].
async fn navidrome_moods(
    browse: &BrowseHandle,
    server: &str,
    heard: &[(Vec<String>, Listens)],
    listens: &HashMap<String, Listens>,
    now: u64,
    date: &str,
) -> Result<Vec<Mood>> {
    let (genres, fetched_from) = browse
        .fetch(|c| {
            let server = server_id(&c);
            async move { Ok((c.genres().await?, server)) }
        })
        .await?;
    if fetched_from != server {
        return Err(BrowseError::stale());
    }
    let usable: Vec<&coda_core::navidrome::Genre> = genres
        .iter()
        .filter(|g| check_genre(g.query()).is_ok())
        .collect();
    let sizes: Vec<GenreSize> = usable
        .iter()
        .map(|g| GenreSize {
            name: g.name.clone(),
            albums: g.album_count.map_or(0, |n| n as usize),
        })
        .collect();
    let ranked = moods::rank_genres(&sizes, heard, now);
    let seed = moods::day_seed(date, Some(server));
    let mut fetched: Vec<String> = Vec::new();
    let mut lists: HashMap<String, Vec<coda_core::navidrome::Album>> = HashMap::new();
    for name in ranked.iter().take(MAX_GENRE_FETCHES) {
        let Some(genre) = usable.iter().find(|g| g.name == *name) else {
            continue;
        };
        let query = genre.query().to_owned();
        let page = browse
            .fetch(|c| async move { c.albums_by_genre(&query, 0, GENRE_ALBUMS).await })
            .await?;
        lists.insert(name.clone(), page.albums);
        fetched.push(name.clone());
        let picks = moods::pick_moods(&fetched, |g| ids_of(&lists, g), listens, now, &seed);
        if moods::complete(&picks) {
            break;
        }
    }
    let picks = moods::pick_moods(&fetched, |g| ids_of(&lists, g), listens, now, &seed);
    let albums: HashMap<&str, &coda_core::navidrome::Album> = lists
        .values()
        .flatten()
        .map(|a| (a.id.as_str(), a))
        .collect();
    Ok(picks
        .into_iter()
        .map(|pick| Mood {
            album_count: usable
                .iter()
                .find(|g| g.name == pick.genre)
                .and_then(|g| g.album_count),
            genre: pick.genre,
            albums: pick
                .albums
                .iter()
                .filter_map(|id| albums.get(id.as_str()))
                .map(|a| MoodAlbum {
                    id: a.id.clone(),
                    name: a.name.clone(),
                    artist: a.artist.clone(),
                    artist_id: a.artist_id.clone(),
                    cover_art: a.cover_art.clone(),
                    duration_ms: a.duration.map(|d| d.as_millis() as u64),
                })
                .collect(),
        })
        .collect())
}

fn ids_of(lists: &HashMap<String, Vec<coda_core::navidrome::Album>>, genre: &str) -> Vec<String> {
    lists
        .get(genre)
        .map(|albums| albums.iter().map(|a| a.id.clone()).collect())
        .unwrap_or_default()
}

/// The cached Navidrome moods as cards.
pub fn navidrome_mood_dtos(
    moods: &[Mood],
    listens: &HashMap<String, Listens>,
    covers: &CoverService,
) -> Vec<MoodDto> {
    moods
        .iter()
        .filter(|m| !m.albums.is_empty())
        .map(|mood| MoodDto {
            genre: mood.genre.clone(),
            album_count: mood.album_count,
            albums: mood
                .albums
                .iter()
                .map(|a| MoodAlbumDto {
                    id: a.id.clone(),
                    name: a.name.clone(),
                    artist: a.artist.clone(),
                    artist_id: a.artist_id.clone(),
                    cover_url: a
                        .cover_art
                        .clone()
                        .and_then(|c| covers.url_for_key(coda_core::cover::CoverKey::Navidrome(c))),
                    duration_ms: a.duration_ms,
                    last_played: listens.get(&a.id).map(|l| l.last),
                    ingredient: IngredientDto::from(&Ingredient::Album {
                        artist: a
                            .artist
                            .clone()
                            .unwrap_or_else(|| UNKNOWN_ARTIST.to_owned()),
                        name: a.name.clone(),
                        id: Some(a.id.clone()),
                    }),
                })
                .collect(),
        })
        .collect()
}

/// One of an artist's most played songs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopTrackDto {
    pub track: TrackDto,
    /// Listens counted by this app.
    pub listens: u64,
}

/// "Your most played" on an artist page: up to [`TOP_TRACKS`] of the
/// artist's songs by the listens this app counted, most first; empty
/// without any. Local: `id` is the artist's library id. Navidrome: the
/// current server's songs whose artist id is `id`; a song without an
/// artist id matches when its album artist or artist is named `name`.
#[tauri::command]
pub async fn artist_most_played(
    engine: State<'_, EngineHandle>,
    browse: State<'_, BrowseHandle>,
    source: SourceKind,
    id: String,
    name: String,
) -> Result<Vec<TopTrackDto>> {
    artist_top_tracks(&engine, &browse, source, id, name).await
}

pub async fn artist_top_tracks(
    engine: &EngineHandle,
    browse: &BrowseHandle,
    source: SourceKind,
    id: String,
    name: String,
) -> Result<Vec<TopTrackDto>> {
    match source {
        SourceKind::Local => {
            let listens = engine.local_track_listens().await.map_err(gone)?;
            browse
                .local(move |view, covers| {
                    let artist = local::artist_id(&view, &id)?;
                    Ok(
                        local_artist_top_tracks(view.library, artist, &listens, TOP_TRACKS)
                            .into_iter()
                            .map(|(track, _, listens)| TopTrackDto {
                                track: track_dto(view.library.track(track), Some(&view), covers),
                                listens,
                            })
                            .collect(),
                    )
                })
                .await
        }
        SourceKind::Navidrome => {
            browse.client()?;
            check_id(&id)?;
            let name: String = name.chars().take(1024).collect();
            let top = engine
                .navidrome_top_tracks(id, name)
                .await
                .map_err(gone)?
                .unwrap_or_default();
            Ok(top
                .into_iter()
                .filter_map(|count| navidrome_top_track(count, browse.covers()))
                .collect())
        }
    }
}

/// A counted Navidrome song as a row, from what the history kept.
fn navidrome_top_track(count: TrackCount, covers: &CoverService) -> Option<TopTrackDto> {
    let song = count.track.song?;
    let mut track = Track::navidrome(
        count.track.id,
        TrackMetadata {
            title: Some(song.title),
            artist: song.artist,
            album: song.album,
            album_artist: song.album_artist,
            track_no: song.track_no,
            disc_no: song.disc_no,
            duration: song.duration_ms.map(Duration::from_millis),
            cover_art: song.cover_art,
            ..TrackMetadata::default()
        },
    );
    track.album_id = song.album_id;
    track.artist_id = song.artist_id;
    Some(TopTrackDto {
        track: track_dto(&track, None, covers),
        listens: count.listens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::SourceDto;
    use crate::history::{HeardTrack, SongInfo};

    #[test]
    fn a_counted_navidrome_song_becomes_a_row_with_its_cover_and_links() {
        let covers = CoverService::new(None);
        let count = TrackCount {
            track: HeardTrack {
                source: SourceKind::Navidrome,
                server: Some(server_id(
                    &coda_core::navidrome::Client::new(
                        url::Url::parse("https://music.example/").unwrap(),
                        "listener".into(),
                        coda_core::navidrome::Password::new("pw".into()),
                    )
                    .unwrap(),
                )),
                id: "s1".into(),
                song: Some(SongInfo {
                    title: "Song".into(),
                    album: Some("Album".into()),
                    album_id: Some("al-1".into()),
                    artist_id: Some("ar-1".into()),
                    cover_art: Some("al-1".into()),
                    duration_ms: Some(61_000),
                    ..SongInfo::default()
                }),
            },
            listens: 12,
            last: 1,
        };
        let row = navidrome_top_track(count.clone(), &covers).unwrap();
        assert_eq!(row.listens, 12);
        assert_eq!(row.track.title, "Song");
        assert_eq!(row.track.album_id.as_deref(), Some("al-1"));
        assert_eq!(row.track.artist_id.as_deref(), Some("ar-1"));
        assert_eq!(row.track.duration_ms, Some(61_000));
        assert!(matches!(
            row.track.source,
            SourceDto::Navidrome { ref song_id, ref cover_art }
                if song_id == "s1" && cover_art.as_deref() == Some("al-1")
        ));
        // Without the song's row (never written so): nothing to show.
        let bare = TrackCount {
            track: HeardTrack {
                song: None,
                ..count.track
            },
            ..count
        };
        assert!(navidrome_top_track(bare, &covers).is_none());
    }
}
