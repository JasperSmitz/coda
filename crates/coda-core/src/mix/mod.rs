//! Mixes (M12, `docs/plans/mixes.md`): a saved recipe of genres, artists
//! and albums from one source, resolved to songs each time it plays and
//! played as an endless, balanced shuffle.
//!
//! - [`Mix`], [`MixSource`] and [`Ingredient`]: the recipe, with its caps
//!   ([`MAX_INGREDIENTS`], [`MAX_MIXES`]). Ingredients are stored by name
//!   (and, for Navidrome artists and albums, the server id too), never by
//!   local library ids, which change with every scan.
//! - [`store`]: the mixes file both frontends share
//!   (`$XDG_DATA_HOME/coda/mixes.json`, ADR 0008).
//! - [`Picker`]: which songs come next (balance and variety), pure.
//! - [`local`]: a local mix's ingredients resolved to songs from the
//!   [`Library`](crate::library::Library).
//! - [`ActiveMix`]: the mix that owns the queue, and its top-up.
//!
//! A Navidrome mix records its server as [`crate::navidrome::server_id`]
//! (a hash): no URLs or credentials are stored.

mod active;
pub mod local;
mod picker;
pub mod store;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use active::{
    ActiveMix, Appended, START_TRACKS, StartError, TOP_UP_TRACKS, TOP_UP_WHEN_REMAINING,
};
pub use picker::{MAX_RECENT_WINDOW, Picker};

use crate::model::{MAX_METADATA_CHARS, clean_text};
use crate::navidrome::api::MAX_ID_CHARS;

/// Most ingredients in one mix.
pub const MAX_INGREDIENTS: usize = 50;
/// Most saved mixes.
pub const MAX_MIXES: usize = 500;
/// Longest mix id, in bytes (ids Coda makes are 16 hex digits).
const MAX_ID_BYTES: usize = 64;

/// A mix's id: random, made by Coda ([`MixId::random`]), so two apps
/// adding mixes at once never collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MixId(String);

impl MixId {
    /// A new random id (16 hex digits).
    pub fn random() -> Self {
        let n = crate::queue::Rng::from_entropy().next_u64();
        Self(format!("{n:016x}"))
    }

    /// An id from a file or a frontend: 1 to 64 ASCII letters, digits,
    /// `-` or `_`.
    pub fn parse(id: &str) -> Option<Self> {
        let valid = !id.is_empty()
            && id.len() <= MAX_ID_BYTES
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        valid.then(|| Self(id.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MixId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a mix's songs come from: one source per mix (decision 2).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MixSource {
    Local,
    /// The server the mix was made on: its
    /// [`server_id`](crate::navidrome::server_id).
    Navidrome {
        server: String,
    },
}

/// One part of a recipe.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ingredient {
    /// Local: the genre's name, matched ignoring case. Navidrome: the
    /// server's exact genre value (genres have no id).
    Genre { name: String },
    /// `id`: the Navidrome artist id (always `None` for a local mix). The
    /// name is the fallback when the id no longer exists.
    Artist { name: String, id: Option<String> },
    /// An album by artist and name; `id` as for [`Ingredient::Artist`].
    Album {
        artist: String,
        name: String,
        id: Option<String>,
    },
}

impl Ingredient {
    /// For messages: `genre Prog Rock`, `artist Yes`,
    /// `album Fragile by Yes`.
    pub fn describe(&self) -> String {
        match self {
            Self::Genre { name } => format!("genre {name}"),
            Self::Artist { name, .. } => format!("artist {name}"),
            Self::Album { artist, name, .. } => format!("album {name} by {artist}"),
        }
    }

    /// Cleaned for `source`, or `None` if unusable: names trimmed and
    /// capped like tags (a Navidrome genre keeps its exact value, only
    /// capped), never blank, no control characters; ids dropped for a
    /// local mix and checked like song ids for Navidrome (empty or over
    /// [`MAX_ID_CHARS`]: `None`).
    fn cleaned(self, source: &MixSource) -> Option<Self> {
        let navidrome = matches!(source, MixSource::Navidrome { .. });
        // A bad id (empty, too long, control characters) is no reason to
        // lose the ingredient: its name is the fallback anyway.
        let id = |id: Option<String>| -> Option<String> {
            id.filter(|id| {
                navidrome && !id.is_empty() && id.chars().count() <= MAX_ID_CHARS && valid_text(id)
            })
        };
        match self {
            Self::Genre { name } if navidrome => {
                let exact = (!name.trim().is_empty() && valid_text(&name)).then_some(name)?;
                Some(Self::Genre { name: exact })
            }
            Self::Genre { name } => Some(Self::Genre {
                name: clean_name(name)?,
            }),
            Self::Artist {
                name,
                id: artist_id,
            } => Some(Self::Artist {
                name: clean_name(name)?,
                id: id(artist_id),
            }),
            Self::Album {
                artist,
                name,
                id: album_id,
            } => Some(Self::Album {
                artist: clean_name(artist)?,
                name: clean_name(name)?,
                id: id(album_id),
            }),
        }
    }
}

/// A saved (or quick) mix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mix {
    pub id: MixId,
    pub name: String,
    pub source: MixSource,
    pub ingredients: Vec<Ingredient>,
}

/// Why a mix was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MixError {
    #[error("a mix needs a name")]
    NoName,
    #[error("a mix needs at least one genre, artist or album")]
    NoIngredients,
    #[error("a mix holds at most {MAX_INGREDIENTS} genres, artists and albums")]
    TooManyIngredients,
    #[error("a genre, artist or album in the mix has no usable name")]
    BadIngredient,
    #[error("the mix's server is not recognised")]
    BadServer,
}

impl Mix {
    /// A new mix with a random id, checked like [`Mix::validated`].
    pub fn new(
        name: impl Into<String>,
        source: MixSource,
        ingredients: Vec<Ingredient>,
    ) -> Result<Self, MixError> {
        Self {
            id: MixId::random(),
            name: name.into(),
            source,
            ingredients,
        }
        .validated()
    }

    /// The mix cleaned and checked: the name trimmed and capped like tags
    /// and never blank; a Navidrome source's server shaped like a
    /// [`server_id`](crate::navidrome::server_id); 1 to
    /// [`MAX_INGREDIENTS`] ingredients, each usable, duplicates dropped
    /// (keeping the first).
    pub fn validated(self) -> Result<Self, MixError> {
        let name = clean_name(self.name).ok_or(MixError::NoName)?;
        if let MixSource::Navidrome { server } = &self.source
            && !crate::navidrome::is_server_id(server)
        {
            return Err(MixError::BadServer);
        }
        if self.ingredients.len() > MAX_INGREDIENTS {
            return Err(MixError::TooManyIngredients);
        }
        let mut ingredients: Vec<Ingredient> = Vec::with_capacity(self.ingredients.len());
        for ingredient in self.ingredients {
            let ingredient = ingredient
                .cleaned(&self.source)
                .ok_or(MixError::BadIngredient)?;
            if !ingredients.contains(&ingredient) {
                ingredients.push(ingredient);
            }
        }
        if ingredients.is_empty() {
            return Err(MixError::NoIngredients);
        }
        Ok(Self {
            id: self.id,
            name,
            source: self.source,
            ingredients,
        })
    }
}

/// The active mix as the session saves it (decision 7): a saved mix by its
/// id (read again from the mixes file on restore), a quick mix by its
/// recipe, since it is saved nowhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMix {
    Saved(MixId),
    Quick(Mix),
}

/// "No songs for genre Jazz, artist Yes": the ingredients that found no
/// songs (decision 3). `None` when every one found some.
pub fn no_songs_message<'a>(missing: impl IntoIterator<Item = &'a Ingredient>) -> Option<String> {
    let names: Vec<String> = missing.into_iter().map(Ingredient::describe).collect();
    (!names.is_empty()).then(|| format!("No songs for {}", names.join(", ")))
}

/// Text that is short enough and has no control characters.
fn valid_text(text: &str) -> bool {
    text.chars().count() <= MAX_METADATA_CHARS && !text.chars().any(char::is_control)
}

/// Trimmed and capped like tags; `None` if blank or holding control
/// characters.
fn clean_name(name: String) -> Option<String> {
    clean_text(Some(name)).filter(|n| !n.chars().any(char::is_control))
}

// ----- file format (shared by the mixes file and the session) ---------

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum SourceFile {
    Local,
    Navidrome { server: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum IngredientFile {
    Genre {
        name: String,
    },
    Artist {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Album {
        artist: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
}

#[derive(Debug, Serialize)]
pub(crate) struct MixFileOut<'a> {
    id: &'a str,
    name: &'a str,
    source: SourceFile,
    ingredients: Vec<IngredientFile>,
}

/// Read leniently: ingredients one by one, so one unusable ingredient
/// (say, of a kind a newer Coda added) drops only itself.
#[derive(Debug, Deserialize)]
struct MixFileIn {
    id: String,
    name: String,
    source: SourceFile,
    ingredients: Vec<serde_json::Value>,
}

impl<'a> MixFileOut<'a> {
    pub(crate) fn of(mix: &'a Mix) -> Self {
        let source = match &mix.source {
            MixSource::Local => SourceFile::Local,
            MixSource::Navidrome { server } => SourceFile::Navidrome {
                server: server.clone(),
            },
        };
        let ingredients = mix
            .ingredients
            .iter()
            .map(|i| match i.clone() {
                Ingredient::Genre { name } => IngredientFile::Genre { name },
                Ingredient::Artist { name, id } => IngredientFile::Artist { name, id },
                Ingredient::Album { artist, name, id } => {
                    IngredientFile::Album { artist, name, id }
                }
            })
            .collect();
        Self {
            id: mix.id.as_str(),
            name: &mix.name,
            source,
            ingredients,
        }
    }
}

/// A mix from untrusted JSON, or `None` if it cannot be used. Unusable
/// ingredients are dropped, and only the first [`MAX_INGREDIENTS`] are
/// kept; a mix left without any is unusable.
pub(crate) fn mix_from_value(value: serde_json::Value) -> Option<Mix> {
    let file: MixFileIn = serde_json::from_value(value).ok()?;
    let source = match file.source {
        SourceFile::Local => MixSource::Local,
        SourceFile::Navidrome { server } => MixSource::Navidrome { server },
    };
    let ingredients = file
        .ingredients
        .into_iter()
        .filter_map(|value| serde_json::from_value::<IngredientFile>(value).ok())
        .map(|i| match i {
            IngredientFile::Genre { name } => Ingredient::Genre { name },
            IngredientFile::Artist { name, id } => Ingredient::Artist { name, id },
            IngredientFile::Album { artist, name, id } => Ingredient::Album { artist, name, id },
        })
        .filter_map(|i| i.cleaned(&source))
        .take(MAX_INGREDIENTS)
        .collect();
    Mix {
        id: MixId::parse(&file.id)?,
        name: file.name,
        source,
        ingredients,
    }
    .validated()
    .ok()
}

/// [`SessionMix`] as the session file holds it: `{"saved": "<id>"}` or
/// `{"quick": <mix>}`.
pub(crate) fn session_mix_to_value(mix: &SessionMix) -> serde_json::Value {
    match mix {
        SessionMix::Saved(id) => serde_json::json!({ "saved": id.as_str() }),
        SessionMix::Quick(mix) => serde_json::json!({ "quick": MixFileOut::of(mix) }),
    }
}

/// A [`SessionMix`] from untrusted JSON; `None` if unusable.
pub(crate) fn session_mix_from_value(value: serde_json::Value) -> Option<SessionMix> {
    let serde_json::Value::Object(mut map) = value else {
        return None;
    };
    if map.len() != 1 {
        return None;
    }
    if let Some(serde_json::Value::String(id)) = map.remove("saved") {
        return MixId::parse(&id).map(SessionMix::Saved);
    }
    map.remove("quick")
        .and_then(mix_from_value)
        .map(SessionMix::Quick)
}

#[cfg(test)]
mod tests;
