//! The active mix (M12, `docs/plans/mixes.md`, ADR 0008): the engine owns
//! it, so topping up never depends on the webview.
//!
//! - **Start:** `playMix` reads the saved mix again from the mixes file
//!   (on a blocking thread; [`Engine::take_mix_load`] and
//!   [`Engine::mix_loaded`]); `playQuickMix` plays a recipe at once. A
//!   local mix resolves its pools from the library inline (a name lookup
//!   per ingredient, like `playLocal`) and starts at once. A Navidrome mix
//!   is **pending** while it loads: it gets a [`MixFetches`] coordinator;
//!   its fetches go out through the loop ([`Engine::take_mix_fetches`],
//!   spawned with `Fetch::spawn`, the current client and the shared fetch
//!   limit, tagged with the mix they are for) and come back to
//!   [`Engine::mix_fetched`]; it starts once they have settled.
//! - **Replacing:** the mix that plays goes on until a new one actually
//!   starts (its queue replaced). Every new play request (`playMix`,
//!   `playQuickMix`, or an intent that replaces the queue) supersedes the
//!   pending mix and a saved mix still being read; a pending mix that
//!   finds no songs (or whose fetches all failed) is dropped with a notice
//!   and the old mix goes on.
//! - **Top-up:** [`Engine::check_mix`] runs after every intent, playback
//!   event, fetch result, retry and library change (a scan batch, also
//!   one applied late on a tick): a mix whose queue was replaced or
//!   cleared has ended and is dropped (with its coordinator, which stops
//!   its fetches); otherwise `wants_top_up` appends songs and the commands
//!   are dispatched (the next track is announced again). A local mix
//!   resolves its pools again when the library changed (a restored mix
//!   waits for the scan this way). A Navidrome mix's due fetches go out
//!   after every check and on the retry timer ([`Engine::mix_retry_at`],
//!   [`Engine::mix_retry`]).
//! - **End:** another server (or none) ends a Navidrome mix, with a notice.
//! - **Session:** the mix that owns the queue is saved with it (its id, or
//!   a quick mix's recipe) and restored on startup, adopting the restored
//!   queue ([`Engine::restore_mix`]). Every change of the playing or
//!   pending mix arms the save debounce, even without a queue change.

use std::path::Path;

use coda_core::mix::navidrome::{Fetch, Fetched, Finished, MixFetches, ResolveError};
use coda_core::mix::store::{self, LoadOutcome, StoreError};
use coda_core::mix::{self, ActiveMix, Mix, MixId, MixSource, Picker, StartError};

use super::*;
use crate::dto::ActiveMixDto;
use crate::mixes::MixInputDto;

/// A mix of the engine: the one playing, or one still loading.
pub(crate) struct MixState {
    /// Tags its fetches, so their results reach this mix only.
    tag: u64,
    active: ActiveMix,
    /// A Navidrome mix's fetches; `None` for a local mix.
    fetches: Option<MixFetches>,
    /// A local mix: the library (generation, tracks) its pools were
    /// resolved from; resolved again when that changes.
    resolved: Option<(u64, usize)>,
}

/// A saved mix being read from the file for `playMix`.
pub(crate) struct MixLoad {
    pub serial: u64,
    pub id: MixId,
    pub path: PathBuf,
    /// The queue context when it was asked for: replaced meanwhile, the
    /// user chose something else and the mix does not start.
    pub context: u64,
}

/// A saved mix as read from the mixes file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReadMix {
    Found(Mix),
    /// Not in the file (deleted, or the file was set aside).
    Gone,
    /// The file was written by a newer Coda: it reads as empty.
    Newer,
}

/// Reads saved mix `id` from `path` (blocking).
pub(crate) fn read_saved_mix(path: &Path, id: &MixId) -> ReadMix {
    let (mixes, outcome) = store::load(path);
    if let LoadOutcome::Newer { .. } = outcome {
        return ReadMix::Newer;
    }
    mixes
        .into_iter()
        .find(|m| &m.id == id)
        .map_or(ReadMix::Gone, ReadMix::Found)
}

/// A saved mix read for `playMix` request `serial`, back from the loop.
#[derive(Debug)]
pub(crate) struct LoadedMix {
    pub serial: u64,
    pub context: u64,
    pub read: ReadMix,
}

const NO_FILE: &str = "Mixes are not available: no data folder";
const GONE: &str = "That mix no longer exists";
const NO_SERVER: &str = "Not connected to Navidrome";
const OTHER_SERVER: &str = "This mix was made on another Navidrome server";

impl Engine {
    /// The mixes file, for `playMix` (set at startup).
    pub fn set_mixes_path(&mut self, path: Option<PathBuf>) {
        self.mixes_path = path;
    }

    /// The mix loading (`loading`), else the one playing.
    pub(crate) fn mix_dto(&self) -> Option<ActiveMixDto> {
        let (state, loading) = match (&self.pending_mix, &self.mix) {
            (Some(state), _) => (state, true),
            (None, Some(state)) => (state, false),
            (None, None) => return None,
        };
        let mix = state.active.mix();
        Some(ActiveMixDto {
            name: mix.name.clone(),
            id: state.active.is_saved().then(|| mix.id.to_string()),
            loading,
        })
    }

    fn next_mix_tag(&mut self) -> u64 {
        self.mix_tags += 1;
        self.mix_tags
    }

    /// The playing or pending mix changed: the snapshot, and a session save
    /// (debounced) even if the queue did not change.
    fn mix_changed(&mut self, now: Instant) {
        self.dirty = true;
        self.saved_revision = None;
        self.save_due.get_or_insert(now + SAVE_DEBOUNCE);
    }

    /// A new valid play request (a mix, a play intent whether or not it
    /// finds songs, or anything else that replaced the queue): the mix
    /// still loading and a saved mix still being read no longer start.
    /// The mix that plays is left alone.
    pub(super) fn supersede_mix_requests(&mut self, now: Instant) {
        self.mix_request += 1;
        self.mix_load = None;
        self.drop_pending_mix(now);
    }

    /// Drops the pending mix (and its coordinator, which stops its
    /// fetches).
    fn drop_pending_mix(&mut self, now: Instant) -> bool {
        let Some(state) = self.pending_mix.take() else {
            return false;
        };
        self.forget_fetches(state.tag);
        self.mix_changed(now);
        true
    }

    /// Drops fetches of mix `tag` not spawned yet.
    fn forget_fetches(&mut self, tag: u64) {
        self.mix_fetch_queue.retain(|(t, _)| *t != tag);
    }

    /// `playMix`: read the saved mix `id` again (the loop does it on a
    /// blocking thread), then start it.
    /// A malformed id or no mixes file changes nothing; a valid request
    /// supersedes the mix loading and the read outstanding.
    pub(super) fn play_saved_mix(&mut self, id: &str, now: Instant) {
        let Some(id) = MixId::parse(id) else {
            self.notify(GONE, now);
            return;
        };
        let Some(path) = self.mixes_path.clone() else {
            self.notify(NO_FILE, now);
            return;
        };
        self.supersede_mix_requests(now);
        self.mix_load = Some(MixLoad {
            serial: self.mix_request,
            id,
            path,
            context: self.player.queue_context(),
        });
    }

    /// The saved mix to read, for the loop.
    pub(crate) fn take_mix_load(&mut self) -> Option<MixLoad> {
        self.mix_load.take()
    }

    /// The saved mix read for a `playMix` request. Dropped if another play
    /// request came since, or the queue was replaced meanwhile.
    pub(crate) fn mix_loaded(&mut self, loaded: LoadedMix, now: Instant) -> Vec<PlaybackCommand> {
        if loaded.serial != self.mix_request
            || loaded.context != self.player.queue_context()
            || self.phase != Phase::Ready
        {
            return Vec::new();
        }
        match loaded.read {
            ReadMix::Found(mix) => {
                let mut commands = self.start_mix(mix, true, now);
                commands.extend(self.check_mix(now));
                commands
            }
            ReadMix::Gone => {
                self.notify(GONE, now);
                Vec::new()
            }
            ReadMix::Newer => {
                self.notify(capitalize(&StoreError::Newer.to_string()), now);
                Vec::new()
            }
        }
    }

    /// `playQuickMix`: a recipe played without saving it. A refused recipe
    /// (no server, or invalid) changes nothing; a valid one supersedes the
    /// mix loading and the read outstanding.
    pub(super) fn play_quick_mix(
        &mut self,
        input: MixInputDto,
        now: Instant,
    ) -> Vec<PlaybackCommand> {
        let source = match input.source {
            crate::history::SourceKind::Local => MixSource::Local,
            crate::history::SourceKind::Navidrome => match &self.client {
                Some(client) => MixSource::Navidrome {
                    server: navidrome::server_id(client),
                },
                None => {
                    self.notify(NO_SERVER, now);
                    return Vec::new();
                }
            },
        };
        let ingredients = input
            .ingredients
            .into_iter()
            .map(mix::Ingredient::from)
            .collect();
        match Mix::new(input.name, source, ingredients) {
            Ok(mix) => {
                self.supersede_mix_requests(now);
                self.start_mix(mix, false, now)
            }
            Err(error) => {
                self.notify(capitalize(&error.to_string()), now);
                Vec::new()
            }
        }
    }

    /// Plays `mix`: a local one starts at once, a Navidrome one is pending
    /// until its first fetches settled. The mix that plays goes on until
    /// then (and if `mix` finds nothing).
    fn start_mix(&mut self, mix: Mix, saved: bool, now: Instant) -> Vec<PlaybackCommand> {
        match mix.source {
            MixSource::Local => {
                let resolved = self.browse.read_library(|view| {
                    let view = view?;
                    let key = (view.generation, view.library.len());
                    Some((mix::local::picker(view.library, &mix), key))
                });
                let Some((picker, key)) = resolved else {
                    self.notify(crate::browse::dto::BrowseError::no_library().message, now);
                    return Vec::new();
                };
                let state = MixState {
                    tag: self.next_mix_tag(),
                    active: ActiveMix::new(mix, saved, picker, &self.player),
                    fetches: None,
                    resolved: Some(key),
                };
                self.start_state(state, now)
            }
            MixSource::Navidrome { .. } => {
                let Some(client) = &self.client else {
                    self.notify(NO_SERVER, now);
                    return Vec::new();
                };
                let fetches = match MixFetches::new(&mix, client) {
                    Ok(fetches) => fetches,
                    Err(error) => {
                        self.notify(error.to_string(), now);
                        return Vec::new();
                    }
                };
                let picker = Picker::new(mix.ingredients.len());
                self.pending_mix = Some(MixState {
                    tag: self.next_mix_tag(),
                    active: ActiveMix::new(mix, saved, picker, &self.player),
                    fetches: Some(fetches),
                    resolved: None,
                });
                self.queue_due_fetches(now);
                self.mix_changed(now);
                Vec::new()
            }
        }
    }

    /// Fills the queue from `state` (a new mix): it then replaces the mix
    /// that played. On failure it is dropped with a notice, and the mix
    /// that played (if any) goes on.
    fn start_state(&mut self, mut state: MixState, now: Instant) -> Vec<PlaybackCommand> {
        let result = state.active.start(&mut self.player, &mut self.rng);
        let missing = mix::no_songs_message(state.active.empty_ingredients());
        let failed: Vec<String> = state
            .fetches
            .as_ref()
            .map(|f| f.failed(&state.active))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|i| state.active.mix().ingredients.get(i))
            .map(mix::Ingredient::describe)
            .collect();
        let name = state.active.mix().name.clone();
        self.mix_changed(now);
        match result {
            Ok(commands) => {
                // The old mix ends here: its queue was replaced.
                if let Some(old) = self.mix.replace(state) {
                    self.forget_fetches(old.tag);
                }
                // Mixes are not in the play history (brief: not in this
                // milestone); the entries no longer count for a context.
                self.played_context(None, now);
                self.notify(format!("Playing mix {name}"), now);
                if let Some(text) = missing {
                    self.notify(text, now);
                }
                if !failed.is_empty() {
                    self.notify(
                        format!("Could not load {}; trying again later", failed.join(", ")),
                        now,
                    );
                }
                self.dispatch(commands, now)
            }
            Err(error) => {
                self.forget_fetches(state.tag);
                match error {
                    StartError::NoSongs => {
                        let text = if failed.is_empty() {
                            missing.unwrap_or_else(|| error.to_string())
                        } else {
                            format!("Could not load the mix's songs ({})", failed.join(", "))
                        };
                        self.notify(text, now);
                    }
                    // The player said why; something else plays already.
                    StartError::Refused | StartError::Ended => {}
                }
                self.dispatch(Vec::new(), now)
            }
        }
    }

    /// Drops the mix that plays (and its coordinator, which stops its
    /// fetches). The session no longer saves it.
    fn end_mix(&mut self, now: Instant) {
        if let Some(state) = self.mix.take() {
            self.forget_fetches(state.tag);
            self.mix_changed(now);
        }
    }

    /// Ends the mix that plays with `text` as a notice.
    fn end_mix_with(&mut self, text: &str, now: Instant) {
        if self.mix.is_some() {
            self.end_mix(now);
            self.notify(text, now);
        }
    }

    /// Drops the pending mix with `text` as a notice.
    fn drop_pending_mix_with(&mut self, text: &str, now: Instant) {
        if self.drop_pending_mix(now) {
            self.notify(text, now);
        }
    }

    /// After every intent, playback event, fetch result, retry and library
    /// change: drops a mix that has ended, tops the queue up and queues
    /// due fetches.
    pub(super) fn check_mix(&mut self, now: Instant) -> Vec<PlaybackCommand> {
        if let Some(state) = &self.pending_mix {
            if state.active.has_ended(&self.player) {
                tracing::debug!("the queue was replaced; the pending mix will not start");
                self.drop_pending_mix(now);
            } else if state.fetches.as_ref().is_some_and(MixFetches::other_server) {
                self.drop_pending_mix_with(OTHER_SERVER, now);
            }
        }
        let commands = self.check_playing_mix(now);
        self.queue_due_fetches(now);
        commands
    }

    fn check_playing_mix(&mut self, now: Instant) -> Vec<PlaybackCommand> {
        let Some(state) = self.mix.as_ref() else {
            return Vec::new();
        };
        if state.active.has_ended(&self.player) {
            tracing::debug!("the queue was replaced; the mix ended");
            self.end_mix(now);
            return Vec::new();
        }
        if state.fetches.as_ref().is_some_and(MixFetches::other_server) {
            self.end_mix_with(OTHER_SERVER, now);
            return Vec::new();
        }
        if state.active.wants_top_up(&self.player).is_some() && state.fetches.is_none() {
            self.refresh_local_pools();
        }
        let Some(state) = self.mix.as_mut() else {
            return Vec::new();
        };
        if state.active.wants_top_up(&self.player).is_none() {
            return Vec::new();
        }
        let appended = state.active.top_up(&mut self.player, &mut self.rng);
        if appended.added == 0 {
            return Vec::new();
        }
        tracing::debug!(added = appended.added, "mix topped up");
        self.dispatch(appended.commands, now)
    }

    /// A local mix's pools from the library again, if it changed since
    /// they were resolved (a scan went on, or a new music folder).
    fn refresh_local_pools(&mut self) {
        let Some(state) = self.mix.as_mut() else {
            return;
        };
        let resolved = state.resolved;
        let mix = state.active.mix().clone();
        let pools = self.browse.read_library(|view| {
            let view = view?;
            let key = (view.generation, view.library.len());
            if Some(key) == resolved {
                return None;
            }
            let pools: Vec<Vec<Track>> = mix
                .ingredients
                .iter()
                .map(|i| mix::local::pool(view.library, i))
                .collect();
            Some((pools, key))
        });
        if let Some((pools, key)) = pools {
            for (i, pool) in pools.into_iter().enumerate() {
                state.active.picker_mut().set_pool(i, pool);
            }
            state.resolved = Some(key);
        }
    }

    /// Queues the fetches the coordinators (pending and playing) say are
    /// due.
    fn queue_due_fetches(&mut self, now: Instant) {
        for state in [self.pending_mix.as_mut(), self.mix.as_mut()]
            .into_iter()
            .flatten()
        {
            if let Some(fetches) = state.fetches.as_mut() {
                let due = fetches.due(&state.active, &self.player, now);
                self.mix_fetch_queue
                    .extend(due.into_iter().map(|f| (state.tag, f)));
            }
        }
    }

    /// The fetches to spawn, each with the tag of its mix, for the loop.
    pub(crate) fn take_mix_fetches(&mut self) -> Vec<(u64, Fetch)> {
        std::mem::take(&mut self.mix_fetch_queue)
    }

    /// The loop could not spawn the fetches (no client): the Navidrome
    /// mixes end.
    pub(crate) fn mix_fetches_unsent(&mut self, now: Instant) {
        let mut ended = false;
        if self
            .pending_mix
            .as_ref()
            .is_some_and(|s| s.fetches.is_some())
        {
            ended |= self.drop_pending_mix(now);
        }
        if self.mix.as_ref().is_some_and(|s| s.fetches.is_some()) {
            self.end_mix(now);
            ended = true;
        }
        if ended {
            self.notify(NO_SERVER, now);
        }
    }

    /// When a failed fetch is due again (the loop's retry timer).
    pub fn mix_retry_at(&self) -> Option<Instant> {
        [self.pending_mix.as_ref(), self.mix.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|s| s.fetches.as_ref()?.next_retry(&s.active))
            .min()
    }

    /// The retry timer fired: send what is due.
    pub fn mix_retry(&mut self, now: Instant) -> Vec<PlaybackCommand> {
        self.check_mix(now)
    }

    /// A result of a fetch for mix `tag`: sets a pool, starts the pending
    /// mix once its fetches settled, or ends a mix on another server. For
    /// a mix that has gone, nothing.
    pub fn mix_fetched(
        &mut self,
        tag: u64,
        fetched: Fetched,
        now: Instant,
    ) -> Vec<PlaybackCommand> {
        if self.pending_mix.as_ref().is_some_and(|s| s.tag == tag) {
            self.pending_fetched(fetched, now)
        } else if self.mix.as_ref().is_some_and(|s| s.tag == tag) {
            self.playing_fetched(fetched, now)
        } else {
            Vec::new()
        }
    }

    fn pending_fetched(&mut self, fetched: Fetched, now: Instant) -> Vec<PlaybackCommand> {
        let Some(state) = self.pending_mix.as_mut() else {
            return Vec::new();
        };
        let Some(fetches) = state.fetches.as_mut() else {
            return Vec::new();
        };
        let finished = fetches.finish(fetched, &mut state.active, &self.player, now);
        tracing::debug!(?finished, "pending mix fetch finished");
        match finished {
            Finished::Stale => return Vec::new(),
            Finished::OtherServer { .. } => {
                self.drop_pending_mix_with(OTHER_SERVER, now);
                return Vec::new();
            }
            _ => {}
        }
        let settled = fetches.settled(&state.active);
        let mut commands = Vec::new();
        if settled && let Some(state) = self.pending_mix.take() {
            commands = self.start_state(state, now);
        }
        commands.extend(self.check_mix(now));
        commands
    }

    fn playing_fetched(&mut self, fetched: Fetched, now: Instant) -> Vec<PlaybackCommand> {
        let Some(state) = self.mix.as_mut() else {
            return Vec::new();
        };
        let Some(fetches) = state.fetches.as_mut() else {
            return Vec::new();
        };
        let finished = fetches.finish(fetched, &mut state.active, &self.player, now);
        tracing::debug!(?finished, "mix fetch finished");
        match finished {
            Finished::Stale => return Vec::new(),
            Finished::OtherServer { .. } => {
                self.end_mix_with(OTHER_SERVER, now);
                return Vec::new();
            }
            Finished::NoSongs { index } => {
                let text = mix::no_songs_message(state.active.mix().ingredients.get(index));
                if let Some(text) = text {
                    self.notify(text, now);
                }
            }
            _ => {}
        }
        self.check_mix(now)
    }

    /// Why a Navidrome mix of `state` cannot go on with the current
    /// client, if it cannot.
    fn off_server(&self, state: &MixState) -> Option<&'static str> {
        let MixSource::Navidrome { server } = &state.active.mix().source else {
            return None;
        };
        match &self.client {
            Some(client) if navidrome::server_id(client) == *server => None,
            Some(_) => Some(OTHER_SERVER),
            None => Some(NO_SERVER),
        }
    }

    /// The client changed: a Navidrome mix (playing or pending) of another
    /// server (or with no server) ends.
    pub(super) fn mix_client_changed(&mut self, now: Instant) {
        if let Some(text) = self.pending_mix.as_ref().and_then(|s| self.off_server(s)) {
            self.drop_pending_mix_with(text, now);
        }
        if let Some(text) = self.mix.as_ref().and_then(|s| self.off_server(s)) {
            self.end_mix_with(text, now);
        }
    }

    /// What the session saves: the mix that owns the queue.
    pub(super) fn session_mix(&self) -> Option<mix::SessionMix> {
        let state = self.mix.as_ref()?;
        state
            .active
            .owns_queue(&self.player)
            .then(|| state.active.session())
    }

    /// The mix saved with the restored session (`saved`: from the mixes
    /// file, else a quick mix), adopting the restored queue. Nothing
    /// without a queue, or for a Navidrome mix without this server.
    pub fn restore_mix(&mut self, restored: Option<(Mix, bool)>, now: Instant) {
        let Some((mix, saved)) = restored else {
            return;
        };
        if self.player.queue().is_empty() {
            return;
        }
        let fetches = match &mix.source {
            MixSource::Local => None,
            MixSource::Navidrome { .. } => {
                let Some(client) = &self.client else {
                    tracing::info!("saved mix not resumed: no Navidrome server");
                    return;
                };
                match MixFetches::new(&mix, client) {
                    Ok(fetches) => Some(fetches),
                    Err(ResolveError::OtherServer) => {
                        tracing::info!("saved mix not resumed: another server");
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "saved mix not resumed");
                        return;
                    }
                }
            }
        };
        // Pools are resolved when the first top-up needs them (a local
        // mix: once the library has songs; Navidrome: fetched now).
        let picker = Picker::new(mix.ingredients.len());
        let mut active = ActiveMix::new(mix, saved, picker, &self.player);
        active.adopt(&self.player);
        self.mix = Some(MixState {
            tag: self.next_mix_tag(),
            active,
            fetches,
            resolved: None,
        });
        self.queue_due_fetches(now);
        self.dirty = true;
    }
}

#[cfg(test)]
impl Engine {
    /// The fetches in flight of the mixes (playing and pending; tests).
    pub(crate) fn mix_fetches_in_flight(&self) -> usize {
        [self.pending_mix.as_ref(), self.mix.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|m| m.fetches.as_ref())
            .map(MixFetches::in_flight)
            .sum()
    }

    /// The tag of the pending mix's fetches (tests).
    pub(crate) fn pending_mix_tag(&self) -> Option<u64> {
        self.pending_mix.as_ref().map(|m| m.tag)
    }

    /// The name of the mix that plays (not a pending one; tests).
    pub(crate) fn playing_mix_name(&self) -> Option<String> {
        self.mix.as_ref().map(|m| m.active.mix().name.clone())
    }

    pub(crate) fn mix_played_through(&self) -> Vec<usize> {
        self.mix
            .as_ref()
            .map_or_else(Vec::new, |m| m.active.picker().played_through())
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
