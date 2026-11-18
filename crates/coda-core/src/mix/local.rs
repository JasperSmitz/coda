//! A local mix's ingredients resolved to songs from the in-memory
//! [`Library`], by name (decision 3): genres, artists and albums are found
//! with the [`TagIndex`](crate::library::tags::TagIndex) lookups that match
//! its own grouping keys (trimmed, ignoring case), so a recipe survives a
//! rescan. Cheap enough to run inline on a frontend's loop.

use super::{Ingredient, Mix, Picker};
use crate::library::Library;
use crate::library::tree::TrackId;
use crate::model::Track;

/// The songs `ingredient` stands for in `library`: a genre's in play
/// order, an artist's albums in turn, an album's tracks. Empty when
/// nothing matches.
pub fn pool(library: &Library, ingredient: &Ingredient) -> Vec<Track> {
    let tags = library.tags();
    let ids: Vec<TrackId> = match ingredient {
        Ingredient::Genre { name } => tags
            .genre_by_name(name)
            .map(|g| tags.genre_tracks(g))
            .unwrap_or_default(),
        Ingredient::Artist { name, .. } => tags
            .artist_by_name(name)
            .map(|a| {
                tags.artist(a)
                    .albums()
                    .iter()
                    .flat_map(|album| tags.album(*album).tracks().iter().copied())
                    .collect()
            })
            .unwrap_or_default(),
        Ingredient::Album { artist, name, .. } => tags
            .artist_by_name(artist)
            .and_then(|a| tags.album_by_name(a, name))
            .map(|album| tags.album(album).tracks().to_vec())
            .unwrap_or_default(),
    };
    ids.into_iter()
        .map(|id| library.track(id).clone())
        .collect()
}

/// A picker over `mix`'s ingredients in `library`, one pool each. For a
/// local mix; the caller checks the source.
pub fn picker(library: &Library, mix: &Mix) -> Picker {
    Picker::from_pools(mix.ingredients.iter().map(|i| pool(library, i)).collect())
}
