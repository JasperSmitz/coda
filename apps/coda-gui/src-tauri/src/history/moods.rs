//! Moods for today (Home): three genres the listener plays a lot, three
//! albums each that they have not heard for longest, then three more
//! genres as Other moods. Pure: the Home command gathers the source's
//! genres and albums (the tag index, or the server) and the history's
//! listens, and caches the day's answer in the history.
//!
//! - **Genres** ([`rank_genres`]): ranked by the listens of their albums
//!   last heard within [`RECENT_DAYS`] (an album counts for each of its
//!   genres), most first; then every other genre by its number of albums
//!   (most first), so a thin history is filled with the source's largest
//!   genres. Ties go by name. Genres are told apart ignoring case.
//! - **Albums** ([`pick_albums`]): never one heard within [`REST_DAYS`],
//!   never one an earlier mood of the day took; never-heard albums first,
//!   in an order drawn from the day's seed, then the least recently heard
//!   (ties by the seed).
//! - **Moods** ([`pick_moods`]): genres in rank order that have
//!   [`MOOD_ALBUMS`] albums to give, up to [`MOODS_KEPT`]; when too few
//!   have that many, the remaining genres with at least one fill in (in
//!   rank order). Everything depends only on the inputs and the seed
//!   (source, server and date), so the moods stay the same all day.

use std::collections::{HashMap, HashSet};

use super::seed;

/// Moods shown at once (today's, and the other ones).
pub const MOODS_SHOWN: usize = 3;
/// Moods kept per day: today's and the other moods.
pub const MOODS_KEPT: usize = 2 * MOODS_SHOWN;
/// Albums per mood.
pub const MOOD_ALBUMS: usize = 3;
/// The genres' ranking counts albums heard within this many days.
pub const RECENT_DAYS: u64 = 56;
/// An album heard within this many days is not picked.
pub const REST_DAYS: u64 = 7;
const DAY: u64 = 24 * 60 * 60;

/// An album's (or a genre's) listens and the last one (unix seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Listens {
    pub listens: u64,
    pub last: u64,
}

/// A genre of the source and how many albums it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenreSize {
    pub name: String,
    pub albums: usize,
}

/// A picked mood: a genre and its albums (keys as given).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    pub genre: String,
    pub albums: Vec<String>,
}

fn fold(name: &str) -> String {
    name.trim().to_lowercase()
}

/// The source's genres in mood order (see the module docs): `heard` is
/// each heard album's genres with its listens.
pub fn rank_genres(
    genres: &[GenreSize],
    heard: &[(Vec<String>, Listens)],
    now: u64,
) -> Vec<String> {
    let since = now.saturating_sub(RECENT_DAYS * DAY);
    let mut scores: HashMap<String, u64> = HashMap::new();
    for (names, listens) in heard {
        if listens.last < since {
            continue;
        }
        let mut seen = HashSet::new();
        for name in names {
            let key = fold(name);
            if seen.insert(key.clone()) {
                let score = scores.entry(key).or_default();
                *score = score.saturating_add(listens.listens);
            }
        }
    }
    let mut seen = HashSet::new();
    let mut ranked: Vec<(u64, usize, &str)> = genres
        .iter()
        .filter(|g| g.albums > 0 && !g.name.trim().is_empty() && seen.insert(fold(&g.name)))
        .map(|g| {
            (
                scores.get(&fold(&g.name)).copied().unwrap_or(0),
                g.albums,
                g.name.as_str(),
            )
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| match (a.0, b.0) {
                // Heard genres by listens only; the rest by size.
                (0, 0) => b.1.cmp(&a.1),
                _ => std::cmp::Ordering::Equal,
            })
            .then_with(|| fold(a.2).cmp(&fold(b.2)))
            .then_with(|| a.2.cmp(b.2))
    });
    ranked.into_iter().map(|(_, _, n)| n.to_owned()).collect()
}

/// Up to [`MOOD_ALBUMS`] of `candidates` (album keys) for a mood: see
/// the module docs. `listens` by album key; `used` the albums earlier
/// moods took; `day` the seed's text (source, server and date).
pub fn pick_albums(
    candidates: &[String],
    listens: &HashMap<String, Listens>,
    used: &HashSet<String>,
    now: u64,
    day: &str,
) -> Vec<String> {
    let rested = now.saturating_sub(REST_DAYS * DAY);
    let mut seen = HashSet::new();
    let mut eligible: Vec<(Option<u64>, u64, &String)> = candidates
        .iter()
        .filter(|key| !used.contains(*key) && seen.insert(*key))
        .filter_map(|key| {
            let last = listens.get(key).map(|l| l.last);
            if last.is_some_and(|last| last > rested) {
                return None;
            }
            Some((last, seed(&format!("{day}\n{key}")), key))
        })
        .collect();
    // `None` (never heard) sorts first, then the oldest last listen.
    eligible.sort();
    eligible
        .into_iter()
        .take(MOOD_ALBUMS)
        .map(|(_, _, key)| key.clone())
        .collect()
}

/// The day's moods (today's first, then the other moods; at most
/// [`MOODS_KEPT`]) from genres in rank order, `albums_of` giving each
/// genre's album keys.
pub fn pick_moods(
    ranked: &[String],
    mut albums_of: impl FnMut(&str) -> Vec<String>,
    listens: &HashMap<String, Listens>,
    now: u64,
    day: &str,
) -> Vec<Picked> {
    let mut used = HashSet::new();
    let mut picked: Vec<Picked> = Vec::new();
    let mut short: Vec<&String> = Vec::new();
    for genre in ranked {
        if picked.len() == MOODS_KEPT {
            break;
        }
        let albums = pick_albums(&albums_of(genre), listens, &used, now, day);
        if albums.len() == MOOD_ALBUMS {
            used.extend(albums.iter().cloned());
            picked.push(Picked {
                genre: genre.clone(),
                albums,
            });
        } else if !albums.is_empty() {
            short.push(genre);
        }
    }
    // Too few full moods: the genres with fewer albums fill in, in rank
    // order, with what is left.
    for genre in short {
        if picked.len() == MOODS_KEPT {
            break;
        }
        let albums = pick_albums(&albums_of(genre), listens, &used, now, day);
        if !albums.is_empty() {
            used.extend(albums.iter().cloned());
            picked.push(Picked {
                genre: genre.clone(),
                albums,
            });
        }
    }
    picked
}

/// Whether `picked` is already the most [`pick_moods`] can give (all
/// moods full): a source whose genres are fetched one by one can stop.
pub fn complete(picked: &[Picked]) -> bool {
    picked.len() == MOODS_KEPT && picked.iter().all(|p| p.albums.len() == MOOD_ALBUMS)
}

/// The seed's text for a day's moods of a scope.
pub fn day_seed(date: &str, server: Option<&str>) -> String {
    format!("moods\n{date}\n{}", server.unwrap_or("local"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000 * DAY;

    fn size(name: &str, albums: usize) -> GenreSize {
        GenreSize {
            name: name.into(),
            albums,
        }
    }

    fn heard(genres: &[&str], listens: u64, days_ago: u64) -> (Vec<String>, Listens) {
        (
            genres.iter().map(|g| (*g).to_owned()).collect(),
            Listens {
                listens,
                last: NOW - days_ago * DAY,
            },
        )
    }

    fn keys(prefix: &str, n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{prefix}{i}")).collect()
    }

    fn listened(entries: &[(&str, u64)]) -> HashMap<String, Listens> {
        entries
            .iter()
            .map(|(key, days_ago)| {
                (
                    (*key).to_owned(),
                    Listens {
                        listens: 1,
                        last: NOW - days_ago * DAY,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn genres_rank_by_recent_listens_then_by_size() {
        let sizes = [
            size("Jazz", 10),
            size("Rock", 40),
            size("Folk", 5),
            size("Pop", 40),
            size("Ambient", 2),
            size("Empty", 0),
        ];
        let heard = [
            heard(&["folk"], 3, 2),
            heard(&["Jazz", "JAZZ"], 2, 10),
            heard(&["Ambient"], 9, RECENT_DAYS + 1),
        ];
        // Heard recently: Folk (3), Jazz (2, once per album despite the
        // case); then the rest by size, ties by name; Ambient's listens are
        // too old to count; an empty genre never shows.
        assert_eq!(
            rank_genres(&sizes, &heard, NOW),
            ["Folk", "Jazz", "Pop", "Rock", "Ambient"]
        );
        // No history at all: the largest genres.
        assert_eq!(
            rank_genres(&sizes, &[], NOW),
            ["Pop", "Rock", "Jazz", "Folk", "Ambient"]
        );
    }

    #[test]
    fn albums_never_heard_come_first_then_the_longest_unheard_never_this_week() {
        let candidates = keys("a", 6);
        let listens = listened(&[("a0", 30), ("a1", 90), ("a2", 3), ("a3", 8)]);
        let none = HashSet::new();
        // Never heard: a4 and a5 (in the day's order), then a1 (90 days),
        // never a2 (3 days: this week).
        let picked = pick_albums(&candidates, &listens, &none, NOW, "day");
        assert_eq!(picked.len(), MOOD_ALBUMS);
        let mut first_two = picked[..2].to_vec();
        first_two.sort();
        assert_eq!(first_two, ["a4", "a5"]);
        assert_eq!(picked[2], "a1");
        assert!(!picked.contains(&"a2".to_owned()));
        // Taken by an earlier mood: skipped.
        let used: HashSet<String> = ["a4".to_owned()].into();
        let picked = pick_albums(&candidates, &listens, &used, NOW, "day");
        assert_eq!(picked[1..], ["a1".to_owned(), "a0".to_owned()]);
        // Heard exactly a week ago (or before): eligible again.
        let listens = listened(&[("b0", REST_DAYS)]);
        assert_eq!(
            pick_albums(&["b0".to_owned()], &listens, &none, NOW, "day"),
            ["b0"]
        );
    }

    #[test]
    fn the_day_decides_the_order_of_never_heard_albums() {
        let candidates = keys("a", 40);
        let none = HashSet::new();
        let empty = HashMap::new();
        let today = pick_albums(&candidates, &empty, &none, NOW, "2026-11-19");
        assert_eq!(
            pick_albums(&candidates, &empty, &none, NOW, "2026-11-19"),
            today
        );
        // The candidates' order does not matter.
        let mut reversed = candidates.clone();
        reversed.reverse();
        assert_eq!(
            pick_albums(&reversed, &empty, &none, NOW, "2026-11-19"),
            today
        );
        assert_ne!(
            pick_albums(&candidates, &empty, &none, NOW, "2026-11-20"),
            today
        );
    }

    #[test]
    fn moods_take_full_genres_first_and_fill_with_smaller_ones() {
        let lists: HashMap<&str, Vec<String>> = HashMap::from([
            ("A", keys("a", 5)),
            ("B", keys("b", 2)),
            ("C", keys("c", 3)),
            ("D", keys("d", 4)),
            ("E", keys("e", 1)),
            // Shares albums with A: none may be picked twice.
            ("F", keys("a", 5)),
        ]);
        let ranked: Vec<String> = ["A", "B", "C", "D", "E", "F"]
            .iter()
            .map(|g| (*g).to_owned())
            .collect();
        let albums_of = |g: &str| lists[g].clone();
        let picked = pick_moods(&ranked, albums_of, &HashMap::new(), NOW, "day");
        let genres: Vec<&str> = picked.iter().map(|p| p.genre.as_str()).collect();
        // Full genres in rank order (A, C, D), then the short ones (B, E)
        // and F with what A left (two albums).
        assert_eq!(genres, ["A", "C", "D", "B", "E", "F"]);
        assert_eq!(picked[3].albums.len(), 2);
        assert_eq!(picked[4].albums.len(), 1);
        assert_eq!(picked[5].albums.len(), 2);
        let all: Vec<&String> = picked.iter().flat_map(|p| &p.albums).collect();
        let distinct: HashSet<&&String> = all.iter().collect();
        assert_eq!(all.len(), distinct.len(), "no album twice");
        assert!(!complete(&picked));
    }

    #[test]
    fn other_moods_are_the_next_three_genres_and_complete_stops_early() {
        let ranked: Vec<String> = (0..8).map(|i| format!("G{i}")).collect();
        let picked = pick_moods(&ranked, |g| keys(g, 3), &HashMap::new(), NOW, "day");
        let genres: Vec<&str> = picked.iter().map(|p| p.genre.as_str()).collect();
        assert_eq!(genres, ["G0", "G1", "G2", "G3", "G4", "G5"]);
        assert!(complete(&picked));
        // A genre whose albums were all heard this week has nothing to give.
        let recent: HashMap<String, Listens> = keys("G0", 3)
            .into_iter()
            .map(|k| {
                (
                    k,
                    Listens {
                        listens: 5,
                        last: NOW,
                    },
                )
            })
            .collect();
        let picked = pick_moods(&ranked, |g| keys(g, 3), &recent, NOW, "day");
        assert_eq!(picked[0].genre, "G1");
    }
}
