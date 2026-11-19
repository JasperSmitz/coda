//! `playLocal`: a local folder, artist or genre played (or shuffled) from the
//! library itself, so a whole-library context never crosses IPC and only
//! the tracks that fit the queue are cloned.

use super::*;
use crate::browse::dto::{BrowseErrorKind, LocalContextDto};
use coda_core::player::context_window;

impl Engine {
    pub(crate) fn play_local(
        &mut self,
        context: LocalContextDto,
        start: Option<usize>,
        shuffle: bool,
        now: Instant,
    ) -> Vec<PlayerCommand> {
        let rng = &mut self.rng;
        // The engine's reads never wait (it is the only writer).
        let picked = self.browse.read_library(|view| {
            let Some(view) = view else {
                return Err(browse::dto::BrowseError::no_library());
            };
            let ids = browse::local::context_track_ids(&view, &context)?;
            let total = ids.len();
            let start = start.filter(|&s| s < total);
            // Each queued track with its place in the context (the stops).
            let (ids, from): (Vec<(usize, _)>, usize) = if shuffle {
                // The start first, then a random sample of the rest.
                let mut rest: Vec<(usize, _)> = ids.into_iter().enumerate().collect();
                let first = start.map(|s| rest.remove(s));
                let room = MAX_QUEUE_LEN - usize::from(first.is_some());
                sample(&mut rest, room, rng);
                (first.into_iter().chain(rest).collect(), 0)
            } else {
                let start = start.unwrap_or(0);
                let window = context_window(total, start.min(total.saturating_sub(1)));
                let from = start.saturating_sub(window.start);
                let first = window.start;
                (
                    ids[window]
                        .iter()
                        .enumerate()
                        .map(|(i, id)| (first + i, *id))
                        .collect(),
                    from,
                )
            };
            let all = view.library.tree().tracks();
            let positions: Vec<usize> = ids.iter().map(|(i, _)| *i).collect();
            let tracks: Vec<Track> = ids.iter().map(|(_, id)| all[id.index()].clone()).collect();
            Ok((tracks, from, total, positions))
        });
        let (tracks, from, total, positions) = match picked {
            Ok(picked) => picked,
            Err(error) => {
                let text = match error.kind {
                    BrowseErrorKind::Stale => "The library changed; reload the view".to_owned(),
                    _ => error.message,
                };
                self.notify(text, now);
                return Vec::new();
            }
        };
        if tracks.is_empty() {
            self.notify("Nothing to play", now);
            return Vec::new();
        }
        let queued = tracks.len();
        let commands = self.player.play_context(tracks, from);
        // Only a play that replaced the queue (`played_context` takes it).
        self.queued_order =
            (!commands.is_empty()).then_some(history::QueuedOrder { positions, total });
        if !commands.is_empty() && queued < total {
            let text = if shuffle {
                format!(
                    "Shuffling {} of {}",
                    coda_core::model::thousands(queued),
                    count(total, "track")
                )
            } else {
                format!(
                    "Queue full; {} not added",
                    coda_core::model::thousands(total - queued)
                )
            };
            self.notify(text, now);
        }
        commands
    }
}
