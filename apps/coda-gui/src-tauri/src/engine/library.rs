//! The local library, written without ever waiting on browse work.
//!
//! Browse commands read the library under the shared lock on blocking
//! threads (see `browse`). The engine is its only writer and uses
//! `try_write`: while a read is running, a scan batch, playlist, scan end
//! or library replacement is queued here and applied on the next event or
//! tick (50 ms) once the lock is free. Ids stay valid meanwhile (the
//! library only grows; a replacement bumps the generation).

use super::*;

/// A change to the shared library, in order.
pub(crate) enum LibraryOp {
    /// A new library (start, music folder change).
    Replace(Option<Box<Library>>),
    Scan(ScanEvent),
    /// The scan channel closed.
    Closed,
}

/// What applying the queued ops means for the UI.
enum Outcome {
    Finished {
        tracks: usize,
        skipped: usize,
        errors: usize,
        playlists: usize,
    },
    Closed {
        tracks: usize,
    },
}

impl Engine {
    /// A new library at `dir` (`None`: none), replacing the old one and
    /// anything still queued for it.
    pub(crate) fn replace_library(&mut self, dir: Option<PathBuf>, now: Instant) {
        self.music_dir = dir.clone();
        self.library_ops.clear();
        self.library_ops
            .push(LibraryOp::Replace(dir.map(|d| Box::new(Library::new(d)))));
        self.apply_library_ops(now);
    }

    pub fn handle_scan_event(&mut self, event: ScanEvent, now: Instant) {
        self.library_ops.push(LibraryOp::Scan(event));
        self.apply_library_ops(now);
    }

    /// The scan channel closed; without `Finished` first, the scan failed.
    pub fn scan_closed(&mut self, now: Instant) {
        self.library_ops.push(LibraryOp::Closed);
        self.apply_library_ops(now);
    }

    /// Changes queued while the library was being read.
    pub fn library_ops_pending(&self) -> usize {
        self.library_ops.len()
    }

    /// Applies the queued ops if the lock is free; else they wait for the
    /// next event or tick.
    pub(crate) fn apply_library_ops(&mut self, now: Instant) {
        if self.library_ops.is_empty() {
            return;
        }
        let Some(mut writer) = self.browse.try_write_local() else {
            tracing::trace!(
                pending = self.library_ops.len(),
                "library busy with a browse read; changes deferred"
            );
            return;
        };
        let mut outcomes = Vec::new();
        for op in self.library_ops.drain(..) {
            match op {
                LibraryOp::Replace(library) => writer.replace(library.map(|l| *l)),
                LibraryOp::Scan(event) => {
                    let Some(library) = writer.library_mut() else {
                        continue;
                    };
                    match event {
                        ScanEvent::Batch(tracks) => library.extend(tracks),
                        ScanEvent::Playlist(file) => {
                            if library.add_playlist(file).is_none() {
                                tracing::debug!("playlist over the cap ignored");
                            }
                        }
                        ScanEvent::Finished {
                            tracks,
                            skipped,
                            errors,
                        } => outcomes.push(Outcome::Finished {
                            tracks,
                            skipped,
                            errors,
                            playlists: library.playlists().len(),
                        }),
                    }
                }
                LibraryOp::Closed => outcomes.push(Outcome::Closed {
                    tracks: writer.library_mut().map_or(0, |l| l.len()),
                }),
            }
        }
        drop(writer);
        for outcome in outcomes {
            self.library_outcome(outcome, now);
        }
        self.dirty = true;
    }

    fn library_outcome(&mut self, outcome: Outcome, now: Instant) {
        match outcome {
            Outcome::Finished {
                tracks,
                skipped,
                errors,
                playlists,
            } => {
                tracing::info!(tracks, skipped, errors, "library scan finished");
                self.scan = ScanState::Finished { skipped, errors };
                // Queued local tracks found by the scan get their ids.
                self.sent_queue = None;
                let mut text = format!("Library scan complete: {}", count(tracks, "track"));
                if playlists > 0 {
                    text.push_str(&format!(", {}", count(playlists, "playlist")));
                }
                if skipped > 0 {
                    text.push_str(&format!(", {skipped} skipped"));
                }
                if errors > 0 {
                    text.push_str(&format!(", {}", count(errors, "error")));
                }
                self.notify(text, now);
            }
            Outcome::Closed { tracks } => {
                if self.scan == ScanState::Scanning {
                    tracing::warn!(tracks, "library scan ended without finishing");
                    self.scan = ScanState::Failed;
                    self.notify(
                        format!(
                            "Library scan failed after {}; see the log",
                            count(tracks, "track")
                        ),
                        now,
                    );
                }
            }
        }
    }
}
