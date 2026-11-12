//! Memory and time of a loaded Navidrome lyrics cache: 20,000 songs with
//! about 1.5 KB of lyrics each, in Latin, Cyrillic and CJK script,
//! measured with a counting allocator (so every allocation counts: hash
//! tables, strings, the index), held after loading and at the peak while
//! loading. `#[ignore]`d; run in release:
//!
//! `cargo test --release -p coda-core --test lyrics_cache_memory -- --ignored --nocapture`

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashSet;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Instant;

use coda_core::lyrics::cache::{Store, file_path};
use serde_json::json;

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
/// The most `LIVE` reached (reset before a load).
static PEAK: AtomicIsize = AtomicIsize::new(0);

fn grew(by: isize) {
    let now = LIVE.fetch_add(by, Ordering::Relaxed) + by;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grew(layout.size() as isize);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(
            new_size as isize - layout.size() as isize,
            Ordering::Relaxed,
        );
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const SONGS: usize = 20_000;
const PER_ALBUM: usize = 12;

const LATIN: &[&str] = &[
    "night",
    "river",
    "golden",
    "Shadow",
    "dancing",
    "Heart’s",
    "fire",
    "“echo”",
    "slowly",
    "morning",
    "Stranger",
    "window",
    "silver",
    "train",
    "falling",
    "à",
    "été",
    "rain",
];
const CYRILLIC: &[&str] = &[
    "ночь",
    "река",
    "золото",
    "Тень",
    "танцы",
    "Сердце",
    "огонь",
    "«эхо»",
    "медленно",
    "утро",
    "Незнакомец",
    "окно",
    "серебро",
    "поезд",
    "падение",
    "дождь",
    "и",
    "я",
];
const CJK: &[&str] = &[
    "夜",
    "川",
    "金色の",
    "影",
    "踊る",
    "心の",
    "火",
    "「こだま」",
    "ゆっくり",
    "朝",
    "見知らぬ人",
    "窓",
    "銀",
    "列車",
    "落ちる",
    "雨",
    "愛",
    "君",
];

/// Each corpus with its queries: no match in its script, an English
/// phrase (no match), and some that match.
type Corpus = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);
const CORPORA: &[Corpus] = &[
    (
        "Latin",
        LATIN,
        &[
            "zzz no match",
            "i love you",
            "golden river night",
            "heart's fire",
            "ÉTÉ",
        ],
    ),
    (
        "Cyrillic",
        CYRILLIC,
        &["жжж нет", "i love you", "ночь река", "СЕРДЦЕ огонь"],
    ),
    (
        "CJK",
        CJK,
        &["存在しない", "i love you", "心の 火", "金色の 夜"],
    ),
];

/// About 1.5 KB of lyrics from `words` in lines of seven words (as the
/// local index's timing test).
fn synthetic_lyrics(words: &[&str], i: usize) -> String {
    let mut out = String::new();
    let mut n = i as u64;
    while out.len() < 1_500 {
        for _ in 0..7 {
            n = n
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            out.push_str(words[(n >> 33) as usize % words.len()]);
            out.push(' ');
        }
        out.pop();
        out.push('\n');
    }
    out.pop();
    out
}

#[test]
#[ignore = "slow; run in release for the memory figure"]
fn twenty_thousand_songs_of_lyrics_in_memory() {
    let mut worst_by_corpus = Vec::new();
    for (name, words, queries) in CORPORA {
        let mut lyric_bytes = 0;
        let albums: Vec<_> = (0..SONGS.div_ceil(PER_ALBUM))
            .map(|a| json!({ "id": format!("al-{a:032x}"), "n": PER_ALBUM, "t": 1 }))
            .collect();
        let songs: Vec<_> = (0..SONGS)
            .map(|i| {
                let ly = synthetic_lyrics(words, i);
                lyric_bytes += ly.len();
                json!({
                    "id": format!("{i:032x}"), "al": format!("al-{:032x}", i / PER_ALBUM),
                    "ti": format!("Song title number {i}"), "ar": format!("Artist {}", i / 120),
                    "alb": format!("Album name {}", i / PER_ALBUM), "aa": format!("Artist {}", i / 120),
                    "arid": format!("ar-{:032x}", i / 120), "c": format!("mf-{i:032x}_0"),
                    "d": 1, "n": i % PER_ALBUM + 1, "ms": 215_000, "t": 2, "ly": ly
                })
            })
            .collect();
        let file = json!({ "v": 1, "refreshed": 2, "albums": albums, "songs": songs });
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            file_path(dir.path(), "k"),
            serde_json::to_vec(&file).unwrap(),
        )
        .unwrap();
        drop(file);
        let file_bytes = std::fs::metadata(file_path(dir.path(), "k")).unwrap().len();

        let before = LIVE.load(Ordering::Relaxed);
        PEAK.store(before, Ordering::Relaxed);
        let start = Instant::now();
        let store = Store::open(dir.path(), "k");
        let loaded = start.elapsed();
        let held = LIVE.load(Ordering::Relaxed) - before;
        let peak = PEAK.load(Ordering::Relaxed) - before;
        assert_eq!(store.status().songs, SONGS);
        assert_eq!(store.status().with_lyrics, SONGS);
        println!(
            "{name}: {SONGS} songs, {:.1} MB of lyrics, file {:.1} MB: loaded in {loaded:?}, \
             {:.1} MB held ({:.2}x the lyrics; estimate {:.1} MB), peak while loading {:.1} MB",
            lyric_bytes as f64 / 1e6,
            file_bytes as f64 / 1e6,
            held as f64 / 1e6,
            held as f64 / lyric_bytes as f64,
            store.heap_bytes() as f64 / 1e6,
            peak as f64 / 1e6,
        );

        // The first search builds the reverse fold table, once per process.
        store.search("warm up", &HashSet::new(), 50);
        let mut worst = std::time::Duration::ZERO;
        for query in *queries {
            let start = Instant::now();
            let found = store.search(query, &HashSet::new(), 50);
            let elapsed = start.elapsed();
            worst = worst.max(elapsed);
            println!("  search {query:?}: {} matches in {elapsed:?}", found.total);
        }
        worst_by_corpus.push(worst);
    }
    let latin = worst_by_corpus[0];
    for worst in &worst_by_corpus {
        assert!(*worst < std::time::Duration::from_secs(5), "{worst:?}");
        assert!(
            *worst <= latin * 4 + std::time::Duration::from_millis(20),
            "{worst:?} vs Latin {latin:?}"
        );
    }
}
