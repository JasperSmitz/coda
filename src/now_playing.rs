//! The Now playing screen's state, without terminal code: whether it is
//! open, and when its transient extras show.
//!
//! - **Next up** ([`NowPlaying::up_next`]) names [`Player::up_next`], the
//!   entry that will actually play next. It shows in the last
//!   [`UP_NEXT_BEFORE_END`] of a track (so throughout a shorter one), and
//!   for [`UP_NEXT_FLASH`] when what comes next changes because of the
//!   queue or the user (a skip, a play, `r`), but not when a track simply
//!   ends and the next one is heard.
//! - **Volume** ([`NowPlaying::volume_shown`]) shows for [`VOLUME_SHOWN`]
//!   after `+` or `-`.
//!
//! The loop's tick redraws while they are due to disappear, so nothing
//! here needs its own timer.

use std::time::{Duration, Instant};

use crate::player::Player;
use crate::queue::{QueueEntry, QueueEntryId};

/// Next up shows when at most this much of the track is left.
pub const UP_NEXT_BEFORE_END: Duration = Duration::from_secs(20);
/// How long Next up shows after what comes next changed.
pub const UP_NEXT_FLASH: Duration = Duration::from_secs(4);
/// How long the volume shows after `+` or `-`.
pub const VOLUME_SHOWN: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
pub struct NowPlaying {
    open: bool,
    /// The volume shows until then.
    volume_until: Option<Instant>,
    /// What [`NowPlaying::follow`] saw last: the entry heard and the one
    /// after it (`None` before the first call).
    seen: Option<(Option<QueueEntryId>, Option<QueueEntryId>)>,
    /// Next up shows until then.
    up_next_until: Option<Instant>,
    /// The user acted since the last [`NowPlaying::follow`].
    user_acted: bool,
}

impl NowPlaying {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    /// `+` or `-` was pressed.
    pub fn volume_changed(&mut self, now: Instant) {
        self.volume_until = Some(now + VOLUME_SHOWN);
    }

    /// Whether the volume shows.
    pub fn volume_shown(&self, now: Instant) -> bool {
        self.volume_until.is_some_and(|until| now < until)
    }

    /// A key was handled: a change of what comes next seen at the next
    /// [`NowPlaying::follow`] is the user's doing (a skip, say), even if
    /// the track heard changed too.
    pub fn user_acted(&mut self) {
        self.user_acted = true;
    }

    /// Notices a change of what comes next (the loop calls this before
    /// every frame). The change shows Next up for [`UP_NEXT_FLASH`], unless
    /// the track heard changed without the user doing anything (a track
    /// ended and the next is heard), and except for the first look.
    pub fn follow(&mut self, player: &Player, now: Instant) {
        let playing = player.playing_entry().map(|entry| entry.id);
        let next = player.up_next().map(|entry| entry.id);
        let user = std::mem::take(&mut self.user_acted);
        if let Some((seen_playing, seen_next)) = self.seen
            && seen_next != next
            && next.is_some()
            && (user || seen_playing == playing)
        {
            self.up_next_until = Some(now + UP_NEXT_FLASH);
        }
        self.seen = Some((playing, next));
    }

    /// The entry Next up names now, if it shows: the one after the track
    /// heard, in the track's last [`UP_NEXT_BEFORE_END`] (with a known
    /// duration) or for a while after it changed.
    pub fn up_next<'a>(&self, player: &'a Player, now: Instant) -> Option<&'a QueueEntry> {
        let next = player.up_next()?;
        // Repeat all with one entry: the same track again says nothing.
        if player
            .playing_entry()
            .is_some_and(|playing| playing.id == next.id)
        {
            return None;
        }
        let flashing = self.up_next_until.is_some_and(|until| now < until);
        let ending = player
            .duration()
            .or_else(|| player.playing_entry().and_then(|e| e.track.duration))
            .is_some_and(|duration| {
                let position = player.position().unwrap_or_default();
                duration.saturating_sub(position) <= UP_NEXT_BEFORE_END
            });
        (flashing || ending).then_some(next)
    }

    /// Forgets transient extras that are over. Returns `true` if one was
    /// showing until now.
    pub fn expire(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for until in [&mut self.volume_until, &mut self.up_next_until] {
            if until.is_some_and(|until| now >= until) {
                *until = None;
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests;
