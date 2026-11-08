//! Scrobbling for Navidrome tracks: the pure rule for when a play counts
//! ([`Listen`]), and a background worker that sends the requests
//! ([`Scrobbler`]).
//!
//! Rules:
//! - When a remote track's load starts, it is reported as now playing
//!   (`submission=false`).
//! - A load is submitted (`submission=true`) once, when the time listened
//!   reaches half its duration or [`MAX_SUBMIT_AFTER`], whichever is
//!   first. Tracks shorter than [`MIN_DURATION`], or of unknown duration,
//!   are never submitted.
//! - Listened time accumulates from position reports while not paused. A
//!   step outside `(0, MAX_PROGRESS_STEP]` (a seek, either way) is not
//!   listening; it only moves the baseline.
//! - Every load is separate: a repeat-one reload is a new [`Listen`].
//!
//! [`Listen`] knows no network and no credentials. [`Scrobbling`] applies
//! it to the player's loads (which load is reported, which duration
//! counts) for both frontends; each frontend owns one next to the (equally
//! credential-free) player and hands its [`ScrobbleRequest`]s to the
//! [`Scrobbler`], which sends them one at a time from a bounded queue.
//! Requests are fire-and-forget: a full queue drops the request, failures
//! are logged by kind only, and only the first failure is reported back.

use std::time::{Duration, SystemTime};

use tokio::sync::{mpsc, oneshot};

use super::{Client, Error};
use crate::model::TrackSource;
use crate::playback::LoadId;
use crate::player::{PlaybackStatus, Player};

/// Tracks shorter than this are never submitted.
pub const MIN_DURATION: Duration = Duration::from_secs(30);
/// A play is submitted after this much listening even if it is less than
/// half of the track.
pub const MAX_SUBMIT_AFTER: Duration = Duration::from_secs(4 * 60);
/// The largest position step that counts as listening. Positions arrive
/// about four times a second; anything larger is a seek.
pub const MAX_PROGRESS_STEP: Duration = Duration::from_secs(2);
/// Requests waiting for the worker; more are dropped.
pub const QUEUE_CAPACITY: usize = 16;

/// How long a track of `duration` must be listened to before it is
/// submitted; `None` if it never is.
pub fn submit_after(duration: Option<Duration>) -> Option<Duration> {
    let duration = duration.filter(|d| *d >= MIN_DURATION)?;
    Some((duration / 2).min(MAX_SUBMIT_AFTER))
}

/// One scrobble call for the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrobbleRequest {
    pub song_id: String,
    pub submission: bool,
    /// When the play started (wall clock); sent with submissions.
    pub started_at: Option<SystemTime>,
}

impl ScrobbleRequest {
    pub fn now_playing(song_id: String) -> Self {
        Self {
            song_id,
            submission: false,
            started_at: None,
        }
    }
}

/// Listening progress of one load of a remote track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listen {
    load: LoadId,
    song_id: String,
    started_at: SystemTime,
    /// The last reported position; the load starts at zero.
    last_position: Duration,
    listened: Duration,
    submitted: bool,
}

impl Listen {
    /// A load of `song_id` that started at `started_at`. Wall-clock time,
    /// not an `Instant`: an `Instant` does not advance while the system
    /// sleeps, so converting one back to a date can be off by the sleep.
    pub fn new(load: LoadId, song_id: String, started_at: SystemTime) -> Self {
        Self {
            load,
            song_id,
            started_at,
            last_position: Duration::ZERO,
            listened: Duration::ZERO,
            submitted: false,
        }
    }

    pub fn load(&self) -> LoadId {
        self.load
    }

    pub fn listened(&self) -> Duration {
        self.listened
    }

    /// The now-playing request for this load.
    pub fn now_playing(&self) -> ScrobbleRequest {
        ScrobbleRequest::now_playing(self.song_id.clone())
    }

    /// Records a position report. `duration` is the track's duration as
    /// far as it is known. Returns the submission, exactly once per load,
    /// when the listened time first reaches [`submit_after`].
    pub fn on_position(
        &mut self,
        position: Duration,
        paused: bool,
        duration: Option<Duration>,
    ) -> Option<ScrobbleRequest> {
        let step = position.checked_sub(self.last_position);
        self.last_position = position;
        match step {
            Some(step) if !paused && !step.is_zero() && step <= MAX_PROGRESS_STEP => {
                self.listened += step;
            }
            _ => return None,
        }
        if self.submitted || submit_after(duration).is_none_or(|after| self.listened < after) {
            return None;
        }
        self.submitted = true;
        Some(ScrobbleRequest {
            song_id: self.song_id.clone(),
            submission: true,
            started_at: Some(self.started_at),
        })
    }
}

/// The scrobble state of a frontend: whether plays are reported, the
/// listen of the current load, and the requests waiting for the
/// [`Scrobbler`]. The frontend calls [`Scrobbling::started`] and
/// [`Scrobbling::progressed`] after the player handled `Started` and
/// `Position`, [`Scrobbling::player_changed`] after every player call, and
/// sends what [`Scrobbling::take_requests`] returns.
#[derive(Debug, Default)]
pub struct Scrobbling {
    /// Report Navidrome plays (configured, and a client exists).
    enabled: bool,
    /// Listening progress of the current load, if it is a remote track
    /// and scrobbling is on.
    listen: Option<Listen>,
    /// The load playing when the client was replaced or scrobbling was
    /// switched on: its listen belonged to the old server (or started
    /// before), so it is never reported (it may be a remote stream signed
    /// by the old client).
    unscrobbled_load: Option<LoadId>,
    /// Requests to send, in order.
    requests: Vec<ScrobbleRequest>,
    /// A failure has been shown; later ones are only logged.
    failure_shown: bool,
}

impl Scrobbling {
    /// Reporting on (`enabled`: configured and a client exists) or off.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    /// Whether plays are reported.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Turns reporting on or off at startup (`enabled`: configured and a
    /// client exists). Off drops the listen in progress.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.listen = None;
        }
    }

    /// Switches reporting while running. On a change, the listen and the
    /// waiting requests are dropped, and the track already playing is not
    /// reported halfway. Returns whether it changed.
    pub fn switch(&mut self, enabled: bool, player: &Player) -> bool {
        if enabled == self.enabled {
            return false;
        }
        self.enabled = enabled;
        self.listen = None;
        self.requests.clear();
        self.unscrobbled_load = player.current_load();
        true
    }

    /// The Navidrome client was replaced (`enabled`: reporting to the new
    /// one). The listen in progress belonged to the old server and is
    /// dropped with the waiting requests; the load playing is never
    /// reported; the next failure is shown again.
    pub fn client_replaced(&mut self, enabled: bool, player: &Player) {
        self.enabled = enabled;
        self.listen = None;
        self.unscrobbled_load = player.current_load();
        self.requests.clear();
        self.failure_shown = false;
    }

    /// The player handled `Started { load }`: if it is the current load
    /// and a remote track, it is reported as now playing and its listening
    /// time counted.
    pub fn started(&mut self, player: &Player, load: LoadId) {
        if !self.enabled
            || player.current_load() != Some(load)
            || self.unscrobbled_load == Some(load)
            || self.listen.as_ref().is_some_and(|l| l.load() == load)
        {
            return;
        }
        let song_id = match player.queue().current_entry().map(|e| &e.track.source) {
            Some(TrackSource::Navidrome { song_id }) => song_id.clone(),
            _ => return,
        };
        // Wall clock, not `Instant`: an `Instant` stops during system
        // sleep, so it cannot be turned back into the date of the play.
        let listen = Listen::new(load, song_id, SystemTime::now());
        self.requests.push(listen.now_playing());
        self.listen = Some(listen);
    }

    /// The player handled a position report: the current load's play is
    /// submitted once enough of it was heard.
    pub fn progressed(&mut self, player: &Player, position: Duration) {
        let status = player.status();
        if !matches!(status, PlaybackStatus::Playing | PlaybackStatus::Paused) {
            return;
        }
        let current_load = player.current_load();
        // The server's duration first: it is what scrobble services judge
        // by, and a stream's reported duration may be an estimate. A zero
        // from the server means unknown, so mpv's value is used then.
        let duration = player
            .queue()
            .current_entry()
            .and_then(|e| e.track.duration)
            .filter(|d| !d.is_zero())
            .or(player.duration());
        let Some(listen) = self
            .listen
            .as_mut()
            .filter(|l| Some(l.load()) == current_load)
        else {
            return;
        };
        if let Some(request) =
            listen.on_position(position, status == PlaybackStatus::Paused, duration)
        {
            self.requests.push(request);
        }
    }

    /// After any player call: a new load, a stop or a backend exit ends the
    /// current listen.
    pub fn player_changed(&mut self, player: &Player) {
        if self
            .listen
            .as_ref()
            .is_some_and(|l| Some(l.load()) != player.current_load())
        {
            self.listen = None;
        }
    }

    /// The requests to send, oldest first.
    pub fn take_requests(&mut self) -> Vec<ScrobbleRequest> {
        std::mem::take(&mut self.requests)
    }

    /// A scrobble failed. True for the first failure since startup or the
    /// last client change: the frontend shows that one; the worker logs
    /// every failure.
    pub fn failed(&mut self) -> bool {
        !std::mem::replace(&mut self.failure_shown, true)
    }
}

/// Sends scrobbles from a bounded queue on one background task, in order.
#[derive(Debug, Clone)]
pub struct Scrobbler {
    requests: mpsc::Sender<ScrobbleRequest>,
}

impl Scrobbler {
    /// Starts the worker. The receiver yields the first failed request's
    /// error (later failures are only logged). The worker ends when every
    /// `Scrobbler` is dropped.
    pub fn spawn(client: Client) -> (Self, oneshot::Receiver<Error>) {
        let (requests, mut queue) = mpsc::channel::<ScrobbleRequest>(QUEUE_CAPACITY);
        let (failure_tx, failure_rx) = oneshot::channel();
        tokio::spawn(async move {
            let mut first_failure = Some(failure_tx);
            while let Some(request) = queue.recv().await {
                match client
                    .scrobble(&request.song_id, request.submission, request.started_at)
                    .await
                {
                    Ok(()) => {
                        tracing::debug!(submission = request.submission, "scrobble sent");
                    }
                    Err(error) => {
                        // Only the detail: `Display` can carry untrusted
                        // server text, `Debug` the server address.
                        tracing::warn!(
                            submission = request.submission,
                            detail = %error.detail(),
                            "scrobble failed"
                        );
                        if let Some(tx) = first_failure.take() {
                            let _ = tx.send(error);
                        }
                    }
                }
            }
        });
        (Self { requests }, failure_rx)
    }

    /// Queues `request` without waiting. Dropped (and logged) if the queue
    /// is full or the worker has gone.
    pub fn send(&self, request: ScrobbleRequest) {
        match self.requests.try_send(request) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(request)) => {
                tracing::warn!(
                    submission = request.submission,
                    "scrobble dropped: queue full"
                );
            }
            Err(mpsc::error::TrySendError::Closed(request)) => {
                tracing::warn!(
                    submission = request.submission,
                    "scrobble dropped: worker stopped"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    fn listen() -> Listen {
        Listen::new(
            LoadId(7),
            "song".into(),
            SystemTime::UNIX_EPOCH + secs(1_000),
        )
    }

    /// Feeds positions from `from` to `to` (exclusive of `from`) in
    /// quarter-second steps, as the backend reports them. Returns the
    /// positions at which a submission was returned.
    fn play(
        listen: &mut Listen,
        from: Duration,
        to: Duration,
        paused: bool,
        duration: Option<Duration>,
    ) -> Vec<Duration> {
        let step = Duration::from_millis(250);
        let mut at = from;
        let mut submitted = Vec::new();
        while at < to {
            at = (at + step).min(to);
            if listen.on_position(at, paused, duration).is_some() {
                submitted.push(at);
            }
        }
        submitted
    }

    #[test]
    fn threshold_is_half_the_duration_capped_at_four_minutes() {
        assert_eq!(submit_after(None), None);
        assert_eq!(submit_after(Some(Duration::ZERO)), None);
        assert_eq!(
            submit_after(Some(MIN_DURATION - Duration::from_millis(1))),
            None
        );
        assert_eq!(submit_after(Some(MIN_DURATION)), Some(secs(15)));
        assert_eq!(submit_after(Some(secs(180))), Some(secs(90)));
        assert_eq!(submit_after(Some(secs(480))), Some(MAX_SUBMIT_AFTER));
        assert_eq!(submit_after(Some(secs(3600))), Some(MAX_SUBMIT_AFTER));
        assert_eq!(submit_after(Some(Duration::MAX)), Some(MAX_SUBMIT_AFTER));
    }

    #[test]
    fn submits_once_at_half_the_track() {
        let mut l = listen();
        let duration = Some(secs(180));
        assert_eq!(
            play(&mut l, secs(0), secs(180), false, duration),
            [secs(90)]
        );
        assert_eq!(l.listened(), secs(180));
        let request = {
            let mut l = listen();
            play(&mut l, secs(0), secs(89), false, duration);
            l.on_position(secs(90), false, duration).unwrap()
        };
        assert_eq!(request.song_id, "song");
        assert!(request.submission);
        assert_eq!(
            request.started_at,
            Some(SystemTime::UNIX_EPOCH + secs(1_000))
        );
        let now_playing = l.now_playing();
        assert!(!now_playing.submission);
        assert_eq!(now_playing.started_at, None);
    }

    #[test]
    fn long_tracks_submit_after_four_minutes() {
        let mut l = listen();
        let submitted = play(&mut l, secs(0), secs(600), false, Some(secs(1200)));
        assert_eq!(submitted, [MAX_SUBMIT_AFTER]);
    }

    #[test]
    fn short_or_unknown_tracks_are_never_submitted() {
        for duration in [None, Some(secs(29)), Some(Duration::ZERO)] {
            let mut l = listen();
            assert!(
                play(&mut l, secs(0), secs(600), false, duration).is_empty(),
                "{duration:?}"
            );
        }
        // A duration that becomes known later still counts what was heard.
        let mut l = listen();
        play(&mut l, secs(0), secs(100), false, None);
        assert!(
            l.on_position(secs(100) + secs(1), false, Some(secs(180)))
                .is_some()
        );
    }

    #[test]
    fn seeks_and_pauses_do_not_count_as_listening() {
        let duration = Some(secs(180));
        let mut l = listen();
        play(&mut l, secs(0), secs(10), false, duration);
        // Seek forward 60 s: the jump is not listening.
        assert_eq!(l.on_position(secs(70), false, duration), None);
        assert_eq!(l.listened(), secs(10));
        // Seek back: nothing either, and progress counts again after it.
        assert_eq!(l.on_position(secs(5), false, duration), None);
        play(&mut l, secs(5), secs(10), false, duration);
        assert_eq!(l.listened(), secs(15));
        // Progress reported while paused is not listening.
        play(&mut l, secs(10), secs(20), true, duration);
        assert_eq!(l.listened(), secs(15));
        // A step just within the bound counts; repeated positions do not.
        assert_eq!(l.on_position(secs(22), false, duration), None);
        assert_eq!(l.listened(), secs(17));
        l.on_position(secs(22), false, duration);
        assert_eq!(l.listened(), secs(17));
        // Seeks with `,` and `.` (5 s) never count, so seeking through a track
        // cannot submit it.
        let mut l = listen();
        let mut at = Duration::ZERO;
        while at < secs(180) {
            at += secs(5);
            assert_eq!(l.on_position(at, false, duration), None);
        }
        assert_eq!(l.listened(), Duration::ZERO);
    }

    #[test]
    fn the_first_report_counts_from_the_start_of_the_load() {
        let mut l = listen();
        l.on_position(Duration::from_millis(300), false, None);
        assert_eq!(l.listened(), Duration::from_millis(300));
        let mut l = listen();
        l.on_position(secs(30), false, None);
        assert_eq!(l.listened(), Duration::ZERO, "a jump, not listening");
    }

    // ----- `Scrobbling`, ported from the TUI's app tests -----------------

    use crate::model::{Track, TrackMetadata};
    use crate::playback::{EndReason, PlaybackEvent};
    use crate::player::{PlayerCommand, RepeatMode};

    fn timed_remote(id: &str, secs: u64) -> Track {
        Track::navidrome(
            id.into(),
            TrackMetadata {
                duration: Some(Duration::from_secs(secs)),
                ..TrackMetadata::default()
            },
        )
    }

    fn local() -> Track {
        Track::local("/m/local.flac".into(), TrackMetadata::default())
    }

    /// A player with `tracks` queued and scrobbling on.
    fn scrobbling(tracks: Vec<Track>) -> (Player, Scrobbling) {
        let mut player = Player::new();
        for track in tracks {
            player.enqueue(track);
        }
        let mut s = Scrobbling::default();
        s.set_enabled(true);
        (player, s)
    }

    fn load_of(commands: &[PlayerCommand]) -> LoadId {
        commands
            .iter()
            .find_map(|c| match c {
                PlayerCommand::Load { load, .. } => Some(*load),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no load in {commands:?}"))
    }

    /// Delivers an event as a frontend does: the player first, then the
    /// scrobble state.
    fn event(p: &mut Player, s: &mut Scrobbling, event: PlaybackEvent) -> Vec<PlayerCommand> {
        let started = match event {
            PlaybackEvent::Started { load } => Some(load),
            _ => None,
        };
        let position = match event {
            PlaybackEvent::Position(position) => Some(position),
            _ => None,
        };
        let commands = p.handle_event(event);
        if let Some(load) = started {
            s.started(p, load);
        }
        if let Some(position) = position {
            s.progressed(p, position);
        }
        s.player_changed(p);
        commands
    }

    /// Plays the first entry and delivers `Started` for it.
    fn play_first(p: &mut Player, s: &mut Scrobbling) -> LoadId {
        let id = p.queue().entries()[0].id;
        let commands = p.play_entry(id);
        s.player_changed(p);
        let load = load_of(&commands);
        event(p, s, PlaybackEvent::Started { load });
        load
    }

    /// Reports positions every 250 ms from `from` to `to` seconds.
    fn progress(p: &mut Player, s: &mut Scrobbling, from: f64, to: f64) {
        let mut at = from;
        while at < to {
            at = (at + 0.25).min(to);
            event(p, s, PlaybackEvent::Position(Duration::from_secs_f64(at)));
        }
    }

    fn summary(requests: &[ScrobbleRequest]) -> Vec<String> {
        requests
            .iter()
            .map(|r| {
                let kind = if r.submission { "submit" } else { "playing" };
                format!("{kind} {}", r.song_id)
            })
            .collect()
    }

    #[test]
    fn remote_plays_are_reported_and_submitted_once_at_half() {
        let (mut p, mut s) = scrobbling(vec![timed_remote("s1", 180), local()]);
        let before = SystemTime::now();
        let load = play_first(&mut p, &mut s);
        let after = SystemTime::now();
        assert_eq!(summary(&s.take_requests()), ["playing s1"]);
        progress(&mut p, &mut s, 0.0, 89.75);
        assert!(s.take_requests().is_empty());
        progress(&mut p, &mut s, 89.75, 90.0);
        let requests = s.take_requests();
        assert_eq!(summary(&requests), ["submit s1"]);
        // The wall-clock start of the play, taken at `Started`.
        let started_at = requests[0].started_at.unwrap();
        assert!(before <= started_at && started_at <= after);
        progress(&mut p, &mut s, 90.0, 180.0);
        assert!(s.take_requests().is_empty(), "once per load");
        // The local track after it is never scrobbled.
        let commands = event(
            &mut p,
            &mut s,
            PlaybackEvent::TrackEnded {
                load,
                reason: EndReason::Eof,
            },
        );
        let load = load_of(&commands);
        event(&mut p, &mut s, PlaybackEvent::Started { load });
        progress(&mut p, &mut s, 0.0, 200.0);
        assert!(s.take_requests().is_empty());
    }

    #[test]
    fn a_zero_server_duration_falls_back_to_the_players() {
        let (mut p, mut s) = scrobbling(vec![timed_remote("z", 0)]);
        let id = p.queue().entries()[0].id;
        let load = load_of(&p.play_entry(id));
        event(
            &mut p,
            &mut s,
            PlaybackEvent::Duration(Some(Duration::from_secs(180))),
        );
        event(&mut p, &mut s, PlaybackEvent::Started { load });
        progress(&mut p, &mut s, 0.0, 89.75);
        assert_eq!(summary(&s.take_requests()), ["playing z"]);
        progress(&mut p, &mut s, 89.75, 90.0);
        assert_eq!(summary(&s.take_requests()), ["submit z"]);
    }

    #[test]
    fn seeks_pauses_and_short_tracks_do_not_submit() {
        let (mut p, mut s) = scrobbling(vec![timed_remote("s1", 180), timed_remote("short", 29)]);
        play_first(&mut p, &mut s);
        s.take_requests();
        // Seeking through the track in 5 s steps is not listening.
        let mut at = 0;
        while at < 180 {
            at += 5;
            event(
                &mut p,
                &mut s,
                PlaybackEvent::Position(Duration::from_secs(at)),
            );
        }
        assert!(s.take_requests().is_empty());
        // Positions while paused do not count either.
        event(&mut p, &mut s, PlaybackEvent::Position(Duration::ZERO));
        p.toggle_pause();
        event(&mut p, &mut s, PlaybackEvent::Paused(true));
        progress(&mut p, &mut s, 0.0, 120.0);
        assert!(s.take_requests().is_empty());
        // Resumed: 90 s of real listening from here submits.
        p.toggle_pause();
        event(&mut p, &mut s, PlaybackEvent::Paused(false));
        progress(&mut p, &mut s, 120.0, 210.0);
        assert_eq!(summary(&s.take_requests()), ["submit s1"]);
        // A short track: now playing, but never submitted.
        let commands = p.play_next();
        s.player_changed(&p);
        let load = load_of(&commands);
        event(&mut p, &mut s, PlaybackEvent::Started { load });
        progress(&mut p, &mut s, 0.0, 29.0);
        assert_eq!(summary(&s.take_requests()), ["playing short"]);
    }

    #[test]
    fn a_new_load_starts_a_new_listen_and_stale_progress_is_ignored() {
        let (mut p, mut s) = scrobbling(vec![timed_remote("a", 60), timed_remote("b", 60)]);
        play_first(&mut p, &mut s);
        progress(&mut p, &mut s, 0.0, 20.0);
        // Next before the submission: the next load has not started, so
        // positions still arriving for the old track count for nothing.
        let commands = p.play_next();
        s.player_changed(&p);
        progress(&mut p, &mut s, 20.0, 40.0);
        assert_eq!(summary(&s.take_requests()), ["playing a"]);
        let load = load_of(&commands);
        event(&mut p, &mut s, PlaybackEvent::Started { load });
        progress(&mut p, &mut s, 0.0, 29.75);
        assert_eq!(summary(&s.take_requests()), ["playing b"]);
        progress(&mut p, &mut s, 29.75, 30.0);
        assert_eq!(summary(&s.take_requests()), ["submit b"]);
        // A duplicate Started for the same load changes nothing.
        event(&mut p, &mut s, PlaybackEvent::Started { load });
        assert!(s.take_requests().is_empty());
    }

    #[test]
    fn repeat_one_reloads_are_scrobbled_again() {
        let (mut p, mut s) = scrobbling(vec![timed_remote("a", 40)]);
        p.set_repeat(RepeatMode::One);
        let mut load = play_first(&mut p, &mut s);
        for round in 0..2 {
            if round > 0 {
                event(&mut p, &mut s, PlaybackEvent::Started { load });
            }
            progress(&mut p, &mut s, 0.0, 40.0);
            assert_eq!(summary(&s.take_requests()), ["playing a", "submit a"]);
            let commands = event(
                &mut p,
                &mut s,
                PlaybackEvent::TrackEnded {
                    load,
                    reason: EndReason::Eof,
                },
            );
            load = load_of(&commands);
        }
    }

    #[test]
    fn nothing_is_reported_while_off() {
        let (mut p, mut s) = scrobbling(vec![timed_remote("a", 60)]);
        s.set_enabled(false);
        assert!(!s.enabled());
        play_first(&mut p, &mut s);
        progress(&mut p, &mut s, 0.0, 60.0);
        assert!(s.take_requests().is_empty());
    }

    #[test]
    fn a_switch_or_client_change_never_reports_the_load_playing() {
        for client_change in [false, true] {
            let (mut p, mut s) = scrobbling(vec![timed_remote("a", 60), timed_remote("b", 60)]);
            let load = play_first(&mut p, &mut s);
            progress(&mut p, &mut s, 0.0, 5.0);
            if client_change {
                s.client_replaced(true, &p);
            } else {
                assert!(s.switch(false, &p));
                assert!(!s.switch(false, &p), "no change");
                assert!(s.switch(true, &p));
            }
            assert!(s.take_requests().is_empty(), "waiting requests dropped");
            assert!(s.enabled());
            // The listen is gone, and a late `Started` does not start one.
            event(&mut p, &mut s, PlaybackEvent::Started { load });
            progress(&mut p, &mut s, 5.0, 60.0);
            assert!(s.take_requests().is_empty(), "{client_change}");
            // The next load is reported.
            let commands = p.play_next();
            s.player_changed(&p);
            event(
                &mut p,
                &mut s,
                PlaybackEvent::Started {
                    load: load_of(&commands),
                },
            );
            assert_eq!(summary(&s.take_requests()), ["playing b"]);
        }
    }

    #[test]
    fn only_the_first_failure_is_shown_until_the_client_changes() {
        let (p, mut s) = scrobbling(Vec::new());
        assert!(s.failed());
        assert!(!s.failed());
        s.switch(false, &p);
        assert!(!s.failed(), "a switch keeps it");
        s.client_replaced(true, &p);
        assert!(s.failed());
        assert!(!s.failed());
    }
}
