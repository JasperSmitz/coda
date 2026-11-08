//! Settings applied live: a new Navidrome client (ported from the TUI's
//! `swap_client`), a new music folder (new library and a rescan), and the
//! cover art and scrobbling switches. The config file and keychain are
//! written first by [`crate::settings`]; only a saved change gets here.

use super::*;

/// A saved settings change. `Debug` hides the client.
pub enum SettingsChange {
    /// Switch to this Navidrome client (`None`: disconnected). `scrobble`
    /// is the config file's setting.
    Client {
        client: Option<Client>,
        scrobble: bool,
    },
    /// Scan this folder (canonical) as the new local library.
    MusicDir(PathBuf),
    CoverArt(bool),
    Scrobble(bool),
}

impl std::fmt::Debug for SettingsChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client { client, scrobble } => f
                .debug_struct("Client")
                .field("client", &client.is_some())
                .field("scrobble", scrobble)
                .finish(),
            Self::MusicDir(_) => f.write_str("MusicDir(<path>)"),
            Self::CoverArt(on) => write!(f, "CoverArt({on})"),
            Self::Scrobble(on) => write!(f, "Scrobble({on})"),
        }
    }
}

/// For the loop: replace the scrobbler and, for a new client, ping it.
pub(crate) struct ClientChange {
    pub client: Option<Client>,
    pub scrobble: bool,
    pub ping: bool,
}

const STILL_STARTING: &str = "Coda is still starting; try again in a moment";

impl Engine {
    /// Applies `change`. Refused until startup is done (startup would
    /// overwrite it).
    pub fn apply_settings(
        &mut self,
        change: SettingsChange,
        now: Instant,
    ) -> Result<Vec<PlaybackCommand>, String> {
        if self.phase != Phase::Ready {
            return Err(STILL_STARTING.into());
        }
        let commands = match change {
            SettingsChange::Client { client, scrobble } => {
                let connected = client.is_some();
                let commands = self.swap_client(client, scrobble, now);
                self.notify(
                    if connected {
                        "Navidrome saved; connecting"
                    } else {
                        "Disconnected from Navidrome"
                    },
                    now,
                );
                commands
            }
            SettingsChange::MusicDir(dir) => {
                self.set_music_dir(dir, now);
                Vec::new()
            }
            SettingsChange::CoverArt(on) => {
                self.covers.set_enabled(on);
                // Every cover URL changes: send the queue again.
                self.sent_queue = None;
                Vec::new()
            }
            SettingsChange::Scrobble(on) => {
                self.scrobble_setting = on;
                let scrobbling = on && self.client.is_some();
                // A track already playing is not reported halfway.
                if self.scrobbling.switch(scrobbling, &self.player) {
                    self.client_change = Some(ClientChange {
                        client: self.client.clone(),
                        scrobble: scrobbling,
                        ping: false,
                    });
                }
                Vec::new()
            }
        };
        self.dirty = true;
        Ok(commands)
    }

    /// Replaces the Navidrome client without touching playback: the track
    /// heard keeps its stream; queued remote tracks resolve with the new
    /// client (or fail without one) at their next load. A listen in
    /// progress belonged to the old server and is dropped.
    fn swap_client(
        &mut self,
        client: Option<Client>,
        scrobble: bool,
        now: Instant,
    ) -> Vec<PlaybackCommand> {
        // New cover tokens for remote covers, fetched with the new client.
        self.covers.set_client(client.clone());
        // Browse commands use the new client; ids and results of the old
        // one are stale (a new client generation).
        self.browse.set_client(client.clone());
        // The history shows this server's entries (and picks) only.
        // A listen in progress keeps the contexts it started with (their
        // server's).
        let server = client.as_ref().map(crate::history::server_id);
        let queue = self.player.queue();
        if server != self.server {
            // Remote entries queued so far belong to the old server.
            self.other_server_entries.extend(
                queue
                    .entries()
                    .iter()
                    .filter(|e| matches!(e.track.source, TrackSource::Navidrome { .. }))
                    .map(|e| e.id),
            );
        }
        self.other_server_entries
            .retain(|id| queue.position(*id).is_some());
        self.server = server;
        self.sent_queue = None;
        self.problems.retain(|p| {
            !matches!(
                p.kind,
                ProblemKind::Navidrome | ProblemKind::NoConfig | ProblemKind::Config
            )
        });
        self.navidrome = NavidromeDto {
            state: if client.is_some() {
                ConnectionDto::Connecting
            } else {
                ConnectionDto::NotConfigured
            },
            message: None,
        };
        self.client = client;
        self.scrobble_setting = scrobble;
        self.scrobbling
            .client_replaced(scrobble && self.client.is_some(), &self.player);
        self.client_change = Some(ClientChange {
            client: self.client.clone(),
            scrobble: self.scrobbling.enabled(),
            ping: true,
        });
        tracing::info!(
            navidrome = self.client.is_some(),
            scrobble = self.scrobbling.enabled(),
            "navidrome client replaced"
        );
        // Stream URLs signed by the old client must not start playing: a
        // remote next track is announced again, and one mpv opened but
        // nobody hears yet is loaded again, with the new client.
        let commands = self.player.client_changed();
        self.dispatch(commands, now)
    }

    /// A new local library at `dir`, scanned from scratch. Queued local
    /// tracks keep playing from their paths.
    fn set_music_dir(&mut self, dir: PathBuf, now: Instant) {
        self.problems
            .retain(|p| !matches!(p.kind, ProblemKind::NoConfig | ProblemKind::Config));
        // A new library generation: ids handed out before are stale.
        self.replace_library(Some(dir.clone()), now);
        // Queued local tracks are named by ids of the new library.
        self.sent_queue = None;
        self.scan = ScanState::Scanning;
        self.rescan = Some(dir);
        self.notify("Scanning the new music folder", now);
    }

    /// The scrobbler and ping to replace, once per change.
    pub(crate) fn take_client_change(&mut self) -> Option<ClientChange> {
        self.client_change.take()
    }

    /// A folder to scan instead of the current one.
    pub fn take_rescan(&mut self) -> Option<PathBuf> {
        self.rescan.take()
    }

    /// The `scrobble` setting of the config file, as last applied.
    pub fn scrobble_setting(&self) -> bool {
        self.scrobble_setting
    }
}

/// Starts what a change needs: a new scan (dropping the old receiver ends
/// the old scan), a ping of the new client and a new scrobbler (dropping
/// the old one lets its worker finish what it has queued).
pub(crate) fn apply_settings_io<S: Sink>(engine: &mut Engine, io: &mut Io<S>) {
    if let Some(dir) = engine.take_rescan() {
        io.scan = Some(scan::spawn_scan(dir));
    }
    if let Some(change) = engine.take_client_change() {
        if change.ping {
            io.ping = change.client.clone().map(spawn_ping);
        }
        (io.scrobbler, io.scrobble_failure) = change
            .client
            .filter(|_| change.scrobble)
            .map(Scrobbler::spawn)
            .unzip();
    }
}
