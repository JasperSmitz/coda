//! Mixes in the TUI (M12, `docs/plans/mixes.md`, ADR 0008): the Mixes
//! views, quick-mix marks and the mix that plays. [`App`] owns the mix, as
//! the GUI's engine does, so topping up never depends on the screen.
//!
//! - **Mixes views:** Home → Mixes (both sources) lists the saved mixes of
//!   that source. Opening the view (or `R` on it) asks the loop to read the
//!   shared mixes file on a blocking thread ([`App::take_mixes_read`]); the
//!   result ([`App::mixes_loaded`]) goes to both browsers, each keeping its
//!   own source's mixes (Navidrome: the current server's). The file is
//!   read again whenever a Mixes view becomes shown, including a switch
//!   between the two sources' Mixes views. Enter or `s` on a mix reads
//!   that mix again by id ([`App::take_mix_load`], on a blocking thread),
//!   so its latest recipe plays, as the GUI's `playMix` does: the result
//!   ([`App::mix_loaded`]) starts it unless another play request came or
//!   the queue was replaced meanwhile; a mix deleted since says so.
//! - **Quick mix:** `m` marks (or unmarks) the selected genre, artist or
//!   album row of the source shown ([`Marks`], one per source; the
//!   Navidrome marks start over with another server or account). Local
//!   search results list songs only: `m` there says marks work in the
//!   library's lists. `M` plays the marks as a quick mix and clears them.
//! - **Start:** a local mix resolves its pools from the library inline and
//!   starts at once. A Navidrome mix is **pending** while it loads: it gets
//!   a [`MixFetches`] coordinator; its fetches go out through the loop
//!   ([`App::take_mix_fetches`], spawned with `Fetch::spawn`, the current
//!   client and the browser's shared fetch limit, never under a permit of
//!   it, tagged with their mix) and come back to [`App::mix_fetched`]; it
//!   starts once they settled. Playing a mix is the latest play request: a
//!   Navidrome play still waiting is dropped. A request refused (no
//!   library, no server) changes nothing.
//! - **Replacing:** the mix that plays goes on until a new one starts (its
//!   queue replaced). Any new valid play request supersedes the pending
//!   mix and a saved mix still being read: another mix, or Enter or `s` on
//!   something that plays (even when it then finds no songs, such as an
//!   empty playlist; not Enter that opens a row), or anything else that
//!   replaces or clears the queue. A pending mix that finds no songs is
//!   dropped with a message and the old mix goes on.
//! - **Top-up:** [`App::check_mix`] runs after every action, playback
//!   event, fetch result (browse or mix), retry and scan batch: a mix whose
//!   queue was replaced or cleared has ended and is dropped (with its
//!   coordinator, which stops its fetches); otherwise `wants_top_up`
//!   appends songs and the commands are dispatched. A local mix resolves
//!   its pools again when the library grew. A Navidrome mix's due fetches
//!   go out after every check and on the retry timer
//!   ([`App::mix_retry_at`], [`App::mix_retry`]).
//! - **End:** another server (or none) ends a Navidrome mix, with a
//!   message.
//! - **Session:** the mix that owns the queue is saved with the TUI's
//!   session (its id, or a quick mix's recipe) and restored at startup,
//!   adopting the restored queue ([`App::restore_mix`]). A mixes file
//!   rejected then is reported in the startup status
//!   ([`restore_saved_mix`]).

use std::path::{Path, PathBuf};

use coda_core::mix::navidrome::{Fetch, Fetched, Finished, MixFetches, ResolveError};
use coda_core::mix::store::{self, LoadOutcome, StoreError};
use coda_core::mix::{
    self, ActiveMix, Ingredient, Marks, Mix, MixId, MixSource, Picker, SessionMix, StartError,
    Toggled,
};

use super::*;

/// A mix of the app: the one playing, or one still loading.
#[derive(Debug)]
pub(super) struct MixState {
    /// Tags its fetches, so their results reach this mix only.
    tag: u64,
    active: ActiveMix,
    /// A Navidrome mix's fetches; `None` for a local mix.
    fetches: Option<MixFetches>,
    /// A local mix: the library size its pools were resolved from; they
    /// are resolved again once the scan added more.
    resolved: Option<usize>,
}

/// A read of the mixes file for the loop ([`App::take_mixes_read`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixesRead {
    pub serial: u64,
    pub path: PathBuf,
}

/// The mixes file as read for request `serial`, back from the loop.
#[derive(Debug)]
pub struct MixesLoaded {
    pub serial: u64,
    pub mixes: Vec<Mix>,
    pub outcome: LoadOutcome,
}

impl MixesRead {
    /// Reads the file (blocking: run on a blocking thread).
    pub fn read(self) -> MixesLoaded {
        let (mixes, outcome) = store::load(&self.path);
        MixesLoaded {
            serial: self.serial,
            mixes,
            outcome,
        }
    }
}

/// A saved mix to read again before it plays (Enter or `s` on a mix row),
/// for the loop ([`App::take_mix_load`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MixLoad {
    pub serial: u64,
    pub id: MixId,
    pub path: PathBuf,
    /// The queue context when it was asked for: replaced meanwhile, the
    /// user chose something else and the mix does not start.
    pub context: u64,
}

/// A saved mix as read from the mixes file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadMix {
    Found(Mix),
    /// Not in the file (deleted).
    Gone,
    /// The file was written by a newer Coda: it reads as empty.
    Newer,
    /// The file could not be used (and was set aside): what to tell the
    /// user.
    Rejected(String),
}

/// The saved mix read for request `serial`, back from the loop.
#[derive(Debug)]
pub struct LoadedMix {
    pub serial: u64,
    pub context: u64,
    pub read: ReadMix,
}

impl MixLoad {
    /// Reads the mix (blocking: run on a blocking thread).
    pub fn read(self) -> LoadedMix {
        LoadedMix {
            serial: self.serial,
            context: self.context,
            read: read_saved_mix(&self.path, &self.id),
        }
    }
}

/// Reads saved mix `id` from `path` (blocking). A rejected file is logged
/// at warn with where it was kept.
fn read_saved_mix(path: &Path, id: &MixId) -> ReadMix {
    let (mixes, outcome) = store::load(path);
    match &outcome {
        LoadOutcome::Newer { .. } => return ReadMix::Newer,
        LoadOutcome::Rejected { kept_as, .. } => {
            tracing::warn!(kept_as = ?kept_as, "the mixes file was rejected");
            return ReadMix::Rejected(outcome.rejected_notice().unwrap_or_default());
        }
        LoadOutcome::Missing | LoadOutcome::Loaded { .. } => {}
    }
    mixes
        .into_iter()
        .find(|m| &m.id == id)
        .map_or(ReadMix::Gone, ReadMix::Found)
}

/// The session's mix as the app resumes it, and a notice for the startup
/// status (a mixes file rejected while reading it).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RestoredMix {
    /// `true`: a saved mix.
    pub mix: Option<(Mix, bool)>,
    pub notice: Option<String>,
}

/// The session's mix as the app resumes it: a saved mix is read again from
/// `path` (blocking), so its latest recipe plays; gone, there is no mix
/// (the queue plays as a plain queue). A rejected file gives a notice.
pub fn restore_saved_mix(session: Option<SessionMix>, path: Option<&Path>) -> RestoredMix {
    let mix = match session {
        None => None,
        Some(SessionMix::Quick(mix)) => Some((mix, false)),
        Some(SessionMix::Saved(id)) => {
            let Some(path) = path else {
                return RestoredMix::default();
            };
            match read_saved_mix(path, &id) {
                ReadMix::Found(mix) => Some((mix, true)),
                ReadMix::Gone => {
                    tracing::info!("the saved mix of the session no longer exists");
                    None
                }
                ReadMix::Newer => {
                    tracing::warn!(
                        "the session's mix not resumed: the mixes file is from a newer Coda"
                    );
                    None
                }
                ReadMix::Rejected(notice) => {
                    return RestoredMix {
                        mix: None,
                        notice: Some(notice),
                    };
                }
            }
        }
    };
    RestoredMix { mix, notice: None }
}

pub const NO_MIXES_FILE: &str = "Mixes are not available: no data folder";
pub const NO_SERVER: &str = "Not connected to Navidrome";
pub const OTHER_SERVER: &str = "This mix was made on another Navidrome server";
pub const NOTHING_MARKED: &str = "Nothing marked: m marks genres, artists and albums";
pub const NOT_MARKABLE: &str = "Only genres, artists and albums can be marked";
pub const MARKS_FULL: &str = "A mix holds at most 50 genres, artists and albums";
pub const GONE: &str = "That mix no longer exists";
pub const MARKS_IN_LISTS: &str =
    "Marks work in the library's lists (genres, artists and albums), not in search results";
pub const NO_ALBUM_ARTIST: &str = "This album has no artist, so it cannot be found again for a mix";

impl App {
    /// The shared mixes file (`None`: no data folder; mixes are then not
    /// available).
    pub fn set_mixes_path(&mut self, path: Option<PathBuf>) {
        self.mixes_path = path;
    }

    /// A read of the mixes file for the loop to run on a blocking thread.
    pub fn take_mixes_read(&mut self) -> Option<MixesRead> {
        self.mixes_read.take()
    }

    /// The mix loading, else the one playing, for the status bar: `Mix:
    /// <name>` or `Quick mix: <name>`, with `(loading…)` while loading.
    pub fn mix_label(&self) -> Option<String> {
        let (state, loading) = match (&self.pending_mix, &self.mix) {
            (Some(state), _) => (state, true),
            (None, Some(state)) => (state, false),
            (None, None) => return None,
        };
        let kind = if state.active.is_saved() {
            "Mix"
        } else {
            "Quick mix"
        };
        let loading = if loading { " (loading…)" } else { "" };
        Some(format!("{kind}: {}{loading}", state.active.mix().name))
    }

    /// The quick-mix marks of `source` (Navidrome: `None` without a
    /// client).
    pub fn marks(&self, source: Source) -> Option<&Marks> {
        match source {
            Source::Local => Some(&self.local_marks),
            Source::Navidrome => self.remote_marks.as_ref(),
        }
    }

    // ----- the Mixes views ---------------------------------------------

    /// The source whose Mixes view is displayed, whatever has the focus.
    /// A change to another one (or from none) reads the file again.
    pub(super) fn mixes_view_displayed(&self) -> Option<Source> {
        let shown = match self.source {
            Source::Navidrome => self
                .browser
                .as_ref()
                .and_then(Browser::current)
                .is_some_and(|v| v.resource == browse::Resource::Mixes),
            Source::Local => {
                self.local_search.is_none()
                    && self.scan != ScanState::NoLibrary
                    && *self.local.current() == LocalView::Mixes
            }
        };
        shown.then_some(self.source)
    }

    /// A Mixes view is shown (in the source shown) and has the focus.
    pub(super) fn mixes_view_shown(&self) -> bool {
        if self.remote_active() {
            self.browser
                .as_ref()
                .and_then(Browser::current)
                .is_some_and(|v| v.resource == browse::Resource::Mixes)
        } else {
            self.local_browse_active() && *self.local.current() == LocalView::Mixes
        }
    }

    /// Asks the loop to read the mixes file again.
    pub(super) fn request_mixes_read(&mut self, now: Instant) {
        let Some(path) = self.mixes_path.clone() else {
            self.set_status(NO_MIXES_FILE, now);
            return;
        };
        self.mixes_serial += 1;
        self.mixes_read = Some(MixesRead {
            serial: self.mixes_serial,
            path,
        });
    }

    /// The mixes file as read: the local mixes to the local browser, this
    /// server's to the Navidrome browser. An older read is dropped.
    pub fn mixes_loaded(&mut self, loaded: MixesLoaded, now: Instant) {
        if loaded.serial != self.mixes_serial {
            return;
        }
        match &loaded.outcome {
            LoadOutcome::Rejected { kept_as, .. } => {
                tracing::warn!(kept_as = ?kept_as, "the mixes file was rejected");
                if let Some(text) = loaded.outcome.rejected_notice() {
                    self.set_status(text, now);
                }
            }
            LoadOutcome::Newer { .. } => {
                self.set_status(capitalize(&StoreError::Newer.to_string()), now)
            }
            LoadOutcome::Missing | LoadOutcome::Loaded { .. } => {}
        }
        let server = self.navidrome.as_ref().map(navidrome::server_id);
        let (local, remote): (Vec<Mix>, Vec<Mix>) = loaded
            .mixes
            .into_iter()
            .filter(|m| match &m.source {
                MixSource::Local => true,
                MixSource::Navidrome { server: s } => server.as_ref() == Some(s),
            })
            .partition(|m| m.source == MixSource::Local);
        self.local.set_mixes(&self.library, local);
        if let Some(browser) = self.browser.as_mut() {
            browser.set_mixes(remote);
        }
    }

    /// The selected saved mix, in a Mixes view of the source shown.
    pub(super) fn selected_mix(&self) -> Option<Mix> {
        if self.remote_active() {
            self.browser.as_ref()?.selected_mix().cloned()
        } else if self.local_browse_active() {
            self.local.selected_mix(&self.library).cloned()
        } else {
            None
        }
    }

    // ----- quick mix ---------------------------------------------------

    /// The selected row as an ingredient and the marks of its source.
    fn selected_for_marking(&mut self) -> Option<(Option<Ingredient>, &mut Marks)> {
        // (Local search results are songs; `toggle_mark` says so.)
        if self.remote_active() {
            let ingredient = self.browser.as_ref()?.selected_ingredient();
            Some((ingredient, self.remote_marks.as_mut()?))
        } else if self.local_browse_active() {
            let ingredient = self.local.selected_ingredient(&self.library);
            Some((ingredient, &mut self.local_marks))
        } else {
            None
        }
    }

    /// `m`: marks the selected genre, artist or album for a quick mix, or
    /// unmarks it.
    pub(super) fn toggle_mark(&mut self, now: Instant) {
        if self.local_results_active() {
            self.set_status(MARKS_IN_LISTS, now);
            return;
        }
        let no_artist = self.remote_active()
            && self
                .browser
                .as_ref()
                .is_some_and(Browser::selected_album_without_artist);
        let Some((ingredient, marks)) = self.selected_for_marking() else {
            return;
        };
        let Some(ingredient) = ingredient else {
            self.set_status(
                if no_artist {
                    NO_ALBUM_ARTIST
                } else {
                    NOT_MARKABLE
                },
                now,
            );
            return;
        };
        let what = ingredient.describe();
        let text = match marks.toggle(ingredient) {
            Toggled::Marked(n) => format!("Marked {what} ({n} marked; M plays them as a mix)"),
            Toggled::Unmarked(0) => format!("Unmarked {what}"),
            Toggled::Unmarked(n) => format!("Unmarked {what} ({n} marked)"),
            Toggled::Full => MARKS_FULL.to_owned(),
            Toggled::Unusable => NOT_MARKABLE.to_owned(),
        };
        self.set_status(text, now);
    }

    /// `M`: plays the marks of the source shown as a quick mix and clears
    /// them.
    pub(super) fn play_marks(&mut self, now: Instant) -> Vec<PlayerCommand> {
        let marks = if self.remote_active() {
            self.remote_marks.as_mut()
        } else if self.local_browse_active() || self.local_results_active() {
            Some(&mut self.local_marks)
        } else {
            None
        };
        let Some(marks) = marks else {
            return Vec::new();
        };
        if marks.is_empty() {
            self.set_status(NOTHING_MARKED, now);
            return Vec::new();
        }
        match marks.quick_mix() {
            Ok(mix) => {
                marks.clear();
                self.play_mix(mix, false, now)
            }
            Err(error) => {
                self.set_status(capitalize(&error.to_string()), now);
                Vec::new()
            }
        }
    }

    // ----- playing a mix -----------------------------------------------

    fn next_mix_tag(&mut self) -> u64 {
        self.mix_tags += 1;
        self.mix_tags
    }

    /// The playing or pending mix changed: the session is saved again
    /// (debounced) even if the queue did not change.
    fn mix_changed(&mut self, now: Instant) {
        self.mix_revision += 1;
        self.note_changes(now);
    }

    /// A new valid play request (a mix, Enter or `s` on something that
    /// plays whether or not it finds songs, or anything else that replaced
    /// the queue): the mix still loading and a saved mix still being read
    /// no longer start. The mix that plays is left alone.
    pub(super) fn supersede_mix_requests(&mut self, now: Instant) {
        self.mix_request += 1;
        self.mix_load = None;
        if let Some(state) = self.pending_mix.take() {
            self.forget_fetches(state.tag);
            self.mix_changed(now);
        }
    }

    /// Drops fetches of mix `tag` not spawned yet.
    fn forget_fetches(&mut self, tag: u64) {
        self.mix_fetch_queue.retain(|(t, _)| *t != tag);
    }

    /// Enter or `s` on a saved mix: the loop reads it again by id
    /// ([`App::take_mix_load`]), then [`App::mix_loaded`] starts it. A
    /// valid request (there is a mixes file) is the latest play request.
    pub(super) fn play_saved_mix(&mut self, id: MixId, now: Instant) -> Vec<PlayerCommand> {
        let Some(path) = self.mixes_path.clone() else {
            self.set_status(NO_MIXES_FILE, now);
            return Vec::new();
        };
        let commands = self.new_mix_request(now);
        self.mix_load = Some(MixLoad {
            serial: self.mix_request,
            id,
            path,
            context: self.player.queue_context(),
        });
        commands
    }

    /// A saved mix to read for the loop (on a blocking thread).
    pub fn take_mix_load(&mut self) -> Option<MixLoad> {
        self.mix_load.take()
    }

    /// The saved mix read for an Enter or `s`. Dropped if another play
    /// request came since, or the queue was replaced meanwhile.
    pub fn mix_loaded(&mut self, loaded: LoadedMix, now: Instant) -> Vec<PlaybackCommand> {
        if loaded.serial != self.mix_request || loaded.context != self.player.queue_context() {
            return Vec::new();
        }
        match loaded.read {
            ReadMix::Found(mix) => {
                self.now_playing.user_acted();
                let commands = self.play_mix(mix, true, now);
                let mut commands = self.dispatch(commands, now);
                commands.extend(self.check_mix(now));
                commands
            }
            ReadMix::Gone => {
                self.set_status(GONE, now);
                self.request_mixes_read(now);
                Vec::new()
            }
            ReadMix::Newer => {
                self.set_status(capitalize(&StoreError::Newer.to_string()), now);
                self.request_mixes_read(now);
                Vec::new()
            }
            ReadMix::Rejected(notice) => {
                self.set_status(notice, now);
                self.request_mixes_read(now);
                Vec::new()
            }
        }
    }

    /// A valid mix request: supersedes the mix loading and a read
    /// outstanding, and drops a Navidrome play still waiting (the adds it
    /// held are applied).
    fn new_mix_request(&mut self, now: Instant) -> Vec<PlayerCommand> {
        self.supersede_mix_requests(now);
        match self.browser.as_mut() {
            Some(browser) => {
                let outcomes = browser.cancel_play();
                self.apply_job_outcomes(outcomes, now)
            }
            None => Vec::new(),
        }
    }

    /// Plays `mix` (`saved`: from the mixes file, else a quick mix): a
    /// local one starts at once, a Navidrome one is pending until its first
    /// fetches settled. The mix that plays goes on until then (and if
    /// `mix` finds nothing). A valid request is the latest play request
    /// ([`App::new_mix_request`]); one refused (no library, no server, not
    /// this server) changes nothing.
    pub(super) fn play_mix(&mut self, mix: Mix, saved: bool, now: Instant) -> Vec<PlayerCommand> {
        match mix.source {
            MixSource::Local => {
                if self.scan == ScanState::NoLibrary {
                    self.set_status(NO_LOCAL_LIBRARY, now);
                    return Vec::new();
                }
                let mut commands = self.new_mix_request(now);
                let picker = mix::local::picker(&self.library, &mix);
                let state = MixState {
                    tag: self.next_mix_tag(),
                    active: ActiveMix::new(mix, saved, picker, &self.player),
                    fetches: None,
                    resolved: Some(self.library.len()),
                };
                commands.extend(self.start_state(state, now));
                commands
            }
            MixSource::Navidrome { .. } => {
                let Some(client) = &self.navidrome else {
                    self.set_status(NO_SERVER, now);
                    return Vec::new();
                };
                let fetches = match MixFetches::new(&mix, client) {
                    Ok(fetches) => fetches,
                    Err(error) => {
                        self.set_status(error.to_string(), now);
                        return Vec::new();
                    }
                };
                let commands = self.new_mix_request(now);
                let name = mix.name.clone();
                let picker = Picker::new(mix.ingredients.len());
                self.pending_mix = Some(MixState {
                    tag: self.next_mix_tag(),
                    active: ActiveMix::new(mix, saved, picker, &self.player),
                    fetches: Some(fetches),
                    resolved: None,
                });
                self.queue_due_fetches(now);
                self.mix_changed(now);
                self.set_status(format!("Loading mix {name}…"), now);
                commands
            }
        }
    }

    /// Fills the queue from `state` (a new mix): it then replaces the mix
    /// that played. On failure it is dropped with a message, and the mix
    /// that played (if any) goes on.
    fn start_state(&mut self, mut state: MixState, now: Instant) -> Vec<PlayerCommand> {
        let result = state.active.start(&mut self.player, &mut self.rng);
        let missing = mix::no_songs_message(state.active.empty_ingredients());
        let failed: Vec<String> = state
            .fetches
            .as_ref()
            .map(|f| f.failed(&state.active))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|i| state.active.mix().ingredients.get(i))
            .map(Ingredient::describe)
            .collect();
        let name = state.active.mix().name.clone();
        self.mix_changed(now);
        match result {
            Ok(commands) => {
                // The old mix ends here: its queue was replaced.
                if let Some(old) = self.mix.replace(state) {
                    self.forget_fetches(old.tag);
                }
                // Like any play that replaces the queue: a Navidrome play
                // still waiting and the adds not applied yet are dropped.
                let dropped = self.local_replace_started(now);
                self.queue_selected = 0;
                let mut text = format!("Playing mix {name}");
                if let Some(missing) = missing {
                    text.push_str(&format!("; {}", lower_first(&missing)));
                }
                if !failed.is_empty() {
                    text.push_str(&format!(
                        "; could not load {}, trying again later",
                        failed.join(", ")
                    ));
                }
                self.set_status(format!("{text}{}", dropped_note(dropped)), now);
                commands
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
                        self.set_status(text, now);
                    }
                    // The player said why; something else plays already.
                    StartError::Refused | StartError::Ended => {}
                }
                Vec::new()
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

    /// After every action, playback event, fetch result, retry and scan
    /// batch: drops a mix that has ended, tops the queue up and queues due
    /// fetches. Returns the commands to send.
    pub(super) fn check_mix(&mut self, now: Instant) -> Vec<PlaybackCommand> {
        if let Some(state) = &self.pending_mix {
            if state.active.has_ended(&self.player) {
                tracing::debug!("the queue was replaced; the pending mix will not start");
                self.supersede_mix_requests(now);
            } else if state.fetches.as_ref().is_some_and(MixFetches::other_server) {
                self.supersede_mix_requests(now);
                self.set_status(OTHER_SERVER, now);
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
            self.end_mix(now);
            self.set_status(OTHER_SERVER, now);
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

    /// A local mix's pools from the library again, if it grew since they
    /// were resolved (the scan went on; a restored mix waits for it).
    fn refresh_local_pools(&mut self) {
        let Some(state) = self.mix.as_mut() else {
            return;
        };
        let size = self.library.len();
        if state.resolved == Some(size) {
            return;
        }
        let pools: Vec<Vec<Track>> = state
            .active
            .mix()
            .ingredients
            .iter()
            .map(|i| mix::local::pool(&self.library, i))
            .collect();
        for (i, pool) in pools.into_iter().enumerate() {
            state.active.picker_mut().set_pool(i, pool);
        }
        state.resolved = Some(size);
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

    /// The mix fetches to spawn, each with the tag of its mix, for the
    /// loop.
    pub fn take_mix_fetches(&mut self) -> Vec<(u64, Fetch)> {
        std::mem::take(&mut self.mix_fetch_queue)
    }

    /// The loop could not spawn the fetches (no client): the Navidrome
    /// mixes end.
    pub fn mix_fetches_unsent(&mut self, now: Instant) {
        let mut ended = false;
        if self
            .pending_mix
            .as_ref()
            .is_some_and(|s| s.fetches.is_some())
        {
            self.supersede_mix_requests(now);
            ended = true;
        }
        if self.mix.as_ref().is_some_and(|s| s.fetches.is_some()) {
            self.end_mix(now);
            ended = true;
        }
        if ended {
            self.set_status(NO_SERVER, now);
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
                self.supersede_mix_requests(now);
                self.set_status(OTHER_SERVER, now);
                return Vec::new();
            }
            _ => {}
        }
        let mut commands = Vec::new();
        if fetches.settled(&state.active)
            && let Some(state) = self.pending_mix.take()
        {
            let started = self.start_state(state, now);
            // A play that waited for this data was the user's doing.
            self.now_playing.user_acted();
            commands = self.dispatch(started, now);
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
                self.end_mix(now);
                self.set_status(OTHER_SERVER, now);
                return Vec::new();
            }
            Finished::NoSongs { index } => {
                let text = mix::no_songs_message(state.active.mix().ingredients.get(index));
                if let Some(text) = text {
                    self.set_status(text, now);
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
        match &self.navidrome {
            Some(client) if navidrome::server_id(client) == *server => None,
            Some(_) => Some(OTHER_SERVER),
            None => Some(NO_SERVER),
        }
    }

    /// The client changed: a Navidrome mix (playing or pending) of another
    /// server (or with no server) ends, and the Navidrome marks start over
    /// for another server or account.
    pub(super) fn mix_client_changed(&mut self, now: Instant) {
        let marks = self.navidrome.as_ref().map(remote_marks);
        if marks.as_ref().map(Marks::source) != self.remote_marks.as_ref().map(Marks::source) {
            self.remote_marks = marks;
        }
        if let Some(text) = self.pending_mix.as_ref().and_then(|s| self.off_server(s)) {
            self.supersede_mix_requests(now);
            self.set_status(text, now);
        }
        if let Some(text) = self.mix.as_ref().and_then(|s| self.off_server(s)) {
            self.end_mix(now);
            self.set_status(text, now);
        }
    }

    /// What the session saves: the mix that owns the queue.
    pub(super) fn session_mix(&self) -> Option<SessionMix> {
        let state = self.mix.as_ref()?;
        state
            .active
            .owns_queue(&self.player)
            .then(|| state.active.session())
    }

    /// The mix saved with the restored session (`saved`: from the mixes
    /// file, else a quick mix), adopting the restored queue; after
    /// [`App::restore_session`]. Nothing without a queue, or for a
    /// Navidrome mix without this server. The restored state still counts
    /// as saved.
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
                let Some(client) = &self.navidrome else {
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
        // It was saved with the session: nothing new to save.
        let saved_before = self.saved_revision == Some(self.seen_revision);
        self.mix_revision += 1;
        self.seen_revision = self.revision();
        if saved_before {
            self.saved_revision = Some(self.seen_revision);
        }
    }
}

/// Empty marks for `client`'s server.
pub(super) fn remote_marks(client: &navidrome::Client) -> Marks {
    Marks::new(MixSource::Navidrome {
        server: navidrome::server_id(client),
    })
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
impl App {
    /// The name of the mix that plays (not a pending one; tests).
    pub(crate) fn playing_mix_name(&self) -> Option<String> {
        self.mix.as_ref().map(|m| m.active.mix().name.clone())
    }

    /// The name of the mix still loading (tests).
    pub(crate) fn pending_mix_name(&self) -> Option<String> {
        self.pending_mix
            .as_ref()
            .map(|m| m.active.mix().name.clone())
    }
}

#[cfg(test)]
#[path = "mix_tests.rs"]
mod tests;
