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
//! [`Listen`] knows no network and no credentials; the app owns it next to
//! the (equally credential-free) player and hands [`ScrobbleRequest`]s to
//! the [`Scrobbler`], which sends them one at a time from a bounded queue.
//! Requests are fire-and-forget: a full queue drops the request, failures
//! are logged by kind only, and only the first failure is reported back.

use std::time::{Duration, SystemTime};

use tokio::sync::{mpsc, oneshot};

use super::{Client, Error};
use crate::playback::LoadId;

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
        // Arrow-key seeks (5 s) never count, so seeking through a track
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
}
