//! Subsonic HTTP client with salted-token authentication.
//!
//! Every request carries `u`, `t = md5(password + s)`, a fresh random salt
//! `s`, `v`, `c` and `f=json`. The plaintext password (`p=`) is never
//! sent. Bodies are read with a size cap. Errors never contain the request
//! URL: every `reqwest::Error` is stripped with `without_url` first.

use std::net::IpAddr;
use std::time::{Duration, SystemTime};

use url::{Host, Url};

use super::api::{self, AUTH_ERROR_CODES, MAX_ALBUM_LIST_SIZE};
use super::{Album, AlbumPage, AlbumSort, Artist, Playlist, PlaylistSongs, SearchResults};
use crate::model::Track;
use crate::playback::StreamUrl;

pub const API_VERSION: &str = "1.16.1";
pub const CLIENT_NAME: &str = "coda";
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Upper bound on a response body (large `getArtists` responses fit).
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// Upper bound on a `getPlaylist` body. A realistic OpenSubsonic entry
/// (several artists, contributors, a long path) is about 2 KB, so 10,000
/// entries take about 20.6 MB (19.7 MiB); this leaves about 20 % headroom.
/// Parsing never builds a `serde_json::Value` (see [`api`]), so memory
/// stays close to the body size.
pub const MAX_PLAYLIST_BODY_BYTES: usize = 24 * 1024 * 1024;
/// Random bytes per salt (hex-encoded, so twice as many characters).
const SALT_BYTES: usize = 12;
pub const SEARCH_ARTIST_COUNT: u32 = 20;
pub const SEARCH_ALBUM_COUNT: u32 = 20;
pub const SEARCH_SONG_COUNT: u32 = 50;

/// A password whose `Debug` and `Display` are redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct Password(String);

impl Password {
    pub fn new(password: String) -> Self {
        Self(password)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Password(<redacted>)")
    }
}

impl std::fmt::Display for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Why a server URL was rejected. Never contains the URL itself.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    #[error("not a valid URL")]
    Invalid,
    #[error("the scheme must be http or https")]
    Scheme,
    #[error("it has no host")]
    NoHost,
    #[error("it must not contain a username or password")]
    Credentials,
    #[error("it must not contain a query or fragment")]
    QueryOrFragment,
}

/// Parses and validates a server base URL; see [`check_base_url`].
pub fn parse_base_url(input: &str) -> Result<Url, UrlError> {
    check_base_url(Url::parse(input.trim()).map_err(|_| UrlError::Invalid)?)
}

/// Requires `http`/`https`, a host, and no userinfo, query or fragment.
/// The path is normalised to end in `/`, so endpoints join as
/// `rest/<name>` below any reverse-proxy prefix.
pub fn check_base_url(mut url: Url) -> Result<Url, UrlError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(UrlError::Scheme);
    }
    if url.host().is_none() {
        return Err(UrlError::NoHost);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UrlError::Credentials);
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(UrlError::QueryOrFragment);
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

/// `true` for plain `http` to a host that is not loopback.
pub fn is_insecure(url: &Url) -> bool {
    if url.scheme() != "http" {
        return false;
    }
    let loopback = match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
        Some(Host::Ipv6(ip)) => IpAddr::V6(ip).is_loopback(),
        None => false,
    };
    !loopback
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Connection, DNS, TLS or timeout failure. The URL has been stripped.
    #[error("{}", if .0.is_timeout() { "server did not respond in time" } else { "could not reach server" })]
    Network(#[source] reqwest::Error),
    #[error("server returned HTTP {0}")]
    Http(u16),
    /// A 3xx answer. Redirects are never followed (see [`Client::new`]);
    /// usually the configured URL is slightly off.
    #[error("server redirected the request; check the url (http vs https, path)")]
    Redirect(u16),
    #[error("authentication failed")]
    Auth,
    #[error("server error {code}: {message}")]
    Server { code: i64, message: String },
    /// The body was not a Subsonic response. The detail is for logs and
    /// contains no response data.
    #[error("unexpected response from server")]
    Decode(String),
    #[error("response too large")]
    TooLarge,
    #[error("invalid server URL: {0}")]
    InvalidUrl(#[from] UrlError),
    #[error("could not set up the HTTP client")]
    Setup(#[source] reqwest::Error),
    #[error("could not generate a random salt")]
    Random,
    /// The fetch task ended without a result (it panicked or was
    /// cancelled).
    #[error("request did not complete")]
    Aborted,
}

impl Error {
    fn network(error: reqwest::Error) -> Self {
        Self::Network(error.without_url())
    }

    /// Extra detail for logs. Contains no response data, URL, host or
    /// secret: network errors are reduced to their kind (their source
    /// chain can name the server's host or address), decode errors to a
    /// category and position.
    pub fn detail(&self) -> String {
        match self {
            Self::Network(error) => {
                let kind = if error.is_timeout() {
                    "timeout"
                } else if error.is_connect() {
                    // TLS handshake failures are reported as connect errors.
                    "connect (including DNS and TLS)"
                } else if error.is_body() || error.is_decode() {
                    "body"
                } else if error.is_request() {
                    "request"
                } else {
                    "other"
                };
                format!("network error: {kind}")
            }
            Self::Http(status) | Self::Redirect(status) => format!("HTTP status {status}"),
            Self::Server { code, .. } => format!("subsonic error code {code}"),
            Self::Decode(detail) => format!("decode: {detail}"),
            Self::Setup(_) => "HTTP client setup failed".to_owned(),
            Self::Auth | Self::TooLarge | Self::InvalidUrl(_) | Self::Random | Self::Aborted => {
                self.to_string()
            }
        }
    }
}

/// Subsonic API client for one server. Cheap to clone (the HTTP client is
/// shared). `Debug` shows nothing but the type name.
#[derive(Clone)]
pub struct Client {
    base: Url,
    username: String,
    password: Password,
    http: reqwest::Client,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Client(<redacted>)")
    }
}

impl Client {
    /// A client with the default timeouts ([`CONNECT_TIMEOUT`],
    /// [`REQUEST_TIMEOUT`]).
    pub fn new(base: Url, username: String, password: Password) -> Result<Self, Error> {
        Self::with_timeouts(base, username, password, CONNECT_TIMEOUT, REQUEST_TIMEOUT)
    }

    pub fn with_timeouts(
        base: Url,
        username: String,
        password: Password,
        connect_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self, Error> {
        let base = check_base_url(base)?;
        if is_insecure(&base) {
            tracing::warn!("Navidrome is configured over plain http; credentials can be observed");
        }
        let http = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(request_timeout)
            // Never follow redirects: reqwest copies the previous URL,
            // signed query included, into the `Referer` header of the
            // redirected request (`make_referer` in reqwest's redirect.rs),
            // handing a replayable token to wherever the server points.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("coda/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Setup(e.without_url()))?;
        Ok(Self {
            base,
            username,
            password,
            http,
        })
    }

    /// The server's base URL (no credentials; may be shown, never logged).
    pub fn base_url(&self) -> &Url {
        &self.base
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    /// Checks reachability and credentials.
    pub async fn ping(&self) -> Result<(), Error> {
        self.get::<api::EmptyReply>("ping", &[]).await.map(drop)
    }

    /// All artists, flattened from the index in served order.
    pub async fn artists(&self) -> Result<Vec<Artist>, Error> {
        let index = self
            .get::<api::ArtistsReply>("getArtists", &[])
            .await?
            .artists
            .ok_or_else(|| Error::Decode("getArtists: missing artists".into()))?;
        Ok(index
            .index
            .into_iter()
            .flat_map(|i| i.artist)
            .filter_map(api::RawArtist::into_artist)
            .collect())
    }

    /// The albums of one artist, in served order.
    pub async fn artist(&self, id: &str) -> Result<Vec<Album>, Error> {
        let artist = self
            .get::<api::ArtistReply>("getArtist", &[("id", id)])
            .await?
            .artist
            .ok_or_else(|| Error::Decode("getArtist: missing artist".into()))?;
        Ok(artist
            .album
            .into_iter()
            .filter_map(api::RawAlbum::into_album)
            .collect())
    }

    /// The songs of one album, in served order.
    pub async fn album(&self, id: &str) -> Result<Vec<Track>, Error> {
        let album = self
            .get::<api::AlbumReply>("getAlbum", &[("id", id)])
            .await?
            .album
            .ok_or_else(|| Error::Decode("getAlbum: missing album".into()))?;
        Ok(album
            .song
            .into_iter()
            .filter_map(api::RawSong::into_track)
            .collect())
    }

    /// One page of albums in `sort` order (`getAlbumList2`), starting at
    /// `offset`. `size` is clamped to `1..=500`.
    pub async fn album_list(
        &self,
        sort: AlbumSort,
        offset: usize,
        size: usize,
    ) -> Result<AlbumPage, Error> {
        let size = size.clamp(1, MAX_ALBUM_LIST_SIZE);
        let size_param = size.to_string();
        let offset_param = offset.to_string();
        let params = [
            ("type", sort.param()),
            ("size", size_param.as_str()),
            ("offset", offset_param.as_str()),
        ];
        let list = self
            .get::<api::AlbumListReply>("getAlbumList2", &params)
            .await?
            .album_list2
            .ok_or_else(|| Error::Decode("getAlbumList2: missing albumList2".into()))?;
        Ok(AlbumPage {
            served: list.album.served.min(size),
            albums: list
                .album
                .items
                .into_iter()
                .take(size)
                .filter_map(api::RawAlbum::into_album)
                .collect(),
        })
    }

    /// The user's playlists, in served order (`getPlaylists`).
    pub async fn playlists(&self) -> Result<Vec<Playlist>, Error> {
        let playlists = self
            .get::<api::PlaylistsReply>("getPlaylists", &[])
            .await?
            .playlists
            .ok_or_else(|| Error::Decode("getPlaylists: missing playlists".into()))?;
        Ok(playlists
            .playlist
            .items
            .into_iter()
            .filter_map(api::RawPlaylist::into_playlist)
            .collect())
    }

    /// The songs of one playlist in playlist order, duplicates included
    /// (`getPlaylist`), with how many entries the server sent.
    pub async fn playlist(&self, id: &str) -> Result<PlaylistSongs, Error> {
        let playlist = self
            .get_capped::<api::PlaylistReply>("getPlaylist", &[("id", id)], MAX_PLAYLIST_BODY_BYTES)
            .await?
            .playlist
            .ok_or_else(|| Error::Decode("getPlaylist: missing playlist".into()))?;
        Ok(PlaylistSongs {
            served: playlist.entry.served,
            tracks: playlist
                .entry
                .items
                .into_iter()
                .filter_map(api::RawSong::into_track)
                .collect(),
        })
    }

    /// `search3` with bounded result counts. A response without a result
    /// section is an empty result.
    pub async fn search(&self, query: &str) -> Result<SearchResults, Error> {
        let artists = SEARCH_ARTIST_COUNT.to_string();
        let albums = SEARCH_ALBUM_COUNT.to_string();
        let songs = SEARCH_SONG_COUNT.to_string();
        let params = [
            ("query", query),
            ("artistCount", artists.as_str()),
            ("albumCount", albums.as_str()),
            ("songCount", songs.as_str()),
        ];
        let result = self
            .get::<api::SearchReply>("search3", &params)
            .await?
            .search_result3
            .unwrap_or_default();
        Ok(SearchResults {
            artists: result
                .artist
                .into_iter()
                .filter_map(api::RawArtist::into_artist)
                .collect(),
            albums: result
                .album
                .into_iter()
                .filter_map(api::RawAlbum::into_album)
                .collect(),
            tracks: result
                .song
                .into_iter()
                .filter_map(api::RawSong::into_track)
                .collect(),
        })
    }

    /// `scrobble`: with `submission` false, reports `id` as now playing;
    /// with `submission` true, records a play. `time` is when the play
    /// started (sent as milliseconds since the Unix epoch); without it the
    /// server uses the time of the request.
    pub async fn scrobble(
        &self,
        id: &str,
        submission: bool,
        time: Option<SystemTime>,
    ) -> Result<(), Error> {
        let time = time
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|since| since.as_millis().to_string());
        let mut params = vec![
            ("id", id),
            ("submission", if submission { "true" } else { "false" }),
        ];
        if let Some(time) = &time {
            params.push(("time", time.as_str()));
        }
        self.get::<api::EmptyReply>("scrobble", &params)
            .await
            .map(drop)
    }

    /// A freshly signed stream URL for `song_id`. Pure apart from the salt.
    /// No transcoding parameters are sent (server defaults apply).
    pub fn stream_url(&self, song_id: &str) -> Result<StreamUrl, Error> {
        let url = self.signed_url("stream", &[("id", song_id)])?;
        StreamUrl::new(url).ok_or(Error::InvalidUrl(UrlError::Scheme))
    }

    fn signed_url(&self, endpoint: &str, params: &[(&str, &str)]) -> Result<Url, Error> {
        let mut url = self
            .base
            .join(&format!("rest/{endpoint}"))
            .map_err(|_| Error::InvalidUrl(UrlError::Invalid))?;
        let salt = salt()?;
        let token = token(self.password.expose(), &salt);
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("u", &self.username)
                .append_pair("t", &token)
                .append_pair("s", &salt)
                .append_pair("v", API_VERSION)
                .append_pair("c", CLIENT_NAME)
                .append_pair("f", "json");
            for (key, value) in params {
                query.append_pair(key, value);
            }
        }
        Ok(url)
    }

    async fn get<R: api::Reply>(
        &self,
        endpoint: &str,
        params: &[(&str, &str)],
    ) -> Result<R, Error> {
        self.get_capped(endpoint, params, MAX_BODY_BYTES).await
    }

    /// Like [`Self::get`], with a body cap of `max_body` bytes.
    async fn get_capped<R: api::Reply>(
        &self,
        endpoint: &str,
        params: &[(&str, &str)],
        max_body: usize,
    ) -> Result<R, Error> {
        let url = self.signed_url(endpoint, params)?;
        tracing::debug!(endpoint, "navidrome request");
        let mut response = self.http.get(url).send().await.map_err(Error::network)?;
        let status = response.status();
        if status.is_redirection() {
            tracing::debug!(
                endpoint,
                status = status.as_u16(),
                "navidrome redirect not followed"
            );
            return Err(Error::Redirect(status.as_u16()));
        }
        if !status.is_success() {
            tracing::debug!(endpoint, status = status.as_u16(), "navidrome HTTP error");
            return Err(Error::Http(status.as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|len| len > max_body as u64)
        {
            return Err(Error::TooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(Error::network)? {
            if body.len() + chunk.len() > max_body {
                return Err(Error::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let response: R = api::parse(&body).map_err(|e| {
            Error::Decode(format!(
                "{endpoint}: {:?} error at line {} column {}",
                e.classify(),
                e.line(),
                e.column()
            ))
        })?;
        check_status(endpoint, response)
    }
}

fn check_status<R: api::Reply>(endpoint: &str, mut response: R) -> Result<R, Error> {
    match response.status() {
        Some("ok") => Ok(response),
        Some("failed") => {
            let error = response.take_error().unwrap_or_default();
            match error.code {
                Some(code) if AUTH_ERROR_CODES.contains(&code) => Err(Error::Auth),
                code => Err(Error::Server {
                    code: code.unwrap_or(0),
                    message: api::clean_message(error.message),
                }),
            }
        }
        _ => Err(Error::Decode(format!(
            "{endpoint}: missing or unknown status"
        ))),
    }
}

/// Lowercase hex md5 of `password + salt`.
fn token(password: &str, salt: &str) -> String {
    let mut input = String::with_capacity(password.len() + salt.len());
    input.push_str(password);
    input.push_str(salt);
    format!("{:x}", md5::compute(input.as_bytes()))
}

fn salt() -> Result<String, Error> {
    let mut bytes = [0u8; SALT_BYTES];
    getrandom::fill(&mut bytes).map_err(|_| Error::Random)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(base: &str) -> Client {
        Client::new(
            parse_base_url(base).unwrap(),
            "me".into(),
            Password::new("sesame".into()),
        )
        .unwrap()
    }

    fn query(url: &str) -> Vec<(String, String)> {
        Url::parse(url)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    #[test]
    fn token_matches_the_subsonic_example() {
        // From the Subsonic API documentation.
        assert_eq!(
            token("sesame", "c19b2d"),
            "26719a1196d2a940705a59634eb18eab"
        );
    }

    #[test]
    fn base_url_validation_and_normalisation() {
        let ok = |s: &str| parse_base_url(s).unwrap().to_string();
        assert_eq!(
            ok("https://music.example.org"),
            "https://music.example.org/"
        );
        assert_eq!(ok("http://h:4533/navidrome"), "http://h:4533/navidrome/");
        assert_eq!(ok("http://h/navidrome/"), "http://h/navidrome/");
        assert_eq!(parse_base_url("ftp://h"), Err(UrlError::Scheme));
        assert_eq!(parse_base_url("file:///x"), Err(UrlError::Scheme));
        assert_eq!(parse_base_url("not a url"), Err(UrlError::Invalid));
        assert_eq!(parse_base_url("https://u:p@h/"), Err(UrlError::Credentials));
        assert_eq!(parse_base_url("https://u@h/"), Err(UrlError::Credentials));
        assert_eq!(
            parse_base_url("https://h/?a=1"),
            Err(UrlError::QueryOrFragment)
        );
        assert_eq!(
            parse_base_url("https://h/#x"),
            Err(UrlError::QueryOrFragment)
        );
        let err = parse_base_url("https://user:hunter2@h/").unwrap_err();
        assert!(!err.to_string().contains("hunter2"));
    }

    #[test]
    fn insecure_detection() {
        let insecure = |s: &str| is_insecure(&parse_base_url(s).unwrap());
        assert!(insecure("http://music.example.org"));
        assert!(insecure("http://192.168.1.2:4533"));
        assert!(!insecure("http://localhost:4533"));
        assert!(!insecure("http://127.0.0.1:4533"));
        assert!(!insecure("http://[::1]:4533"));
        assert!(!insecure("https://music.example.org"));
    }

    #[test]
    fn stream_url_is_signed_and_joined_below_the_prefix() {
        let c = client("https://h.example/navidrome");
        let a = c.stream_url("song 1&x").unwrap();
        let b = c.stream_url("song 1&x").unwrap();
        let url = Url::parse(a.expose()).unwrap();
        assert_eq!(url.path(), "/navidrome/rest/stream");
        let q = query(a.expose());
        let get = |k: &str| q.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        assert_eq!(get("id").as_deref(), Some("song 1&x"));
        assert_eq!(get("u").as_deref(), Some("me"));
        assert_eq!(get("v").as_deref(), Some(API_VERSION));
        assert_eq!(get("c").as_deref(), Some(CLIENT_NAME));
        assert_eq!(get("f").as_deref(), Some("json"));
        assert_eq!(get("p"), None);
        let salt = get("s").unwrap();
        assert!(salt.len() >= 6);
        assert_eq!(get("t").unwrap(), token("sesame", &salt));
        assert_ne!(a, b, "fresh salt per URL");
        assert!(!a.expose().contains("sesame"));
    }

    #[test]
    fn debug_output_is_redacted() {
        let c = client("https://secret-host.example");
        let debug = format!("{c:?}");
        assert!(
            !debug.contains("secret-host") && !debug.contains("sesame"),
            "{debug}"
        );
        let password = Password::new("sesame".into());
        assert_eq!(format!("{password:?}"), "Password(<redacted>)");
        assert_eq!(password.to_string(), "<redacted>");
        let stream = c.stream_url("x").unwrap();
        assert!(!format!("{stream:?}").contains("secret-host"));
    }
}
