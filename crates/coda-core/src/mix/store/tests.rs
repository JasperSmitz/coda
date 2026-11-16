use super::*;
use crate::mix::{Ingredient, MAX_INGREDIENTS, MixSource};
use std::cell::RefCell;

thread_local! {
    /// Run once by this thread's next [`load`] of a bad file, after its
    /// unlocked read and before it takes the exclusive lock.
    static BEFORE_RECHECK: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

pub(super) fn before_recheck() {
    if let Some(hook) = BEFORE_RECHECK.with(|h| h.borrow_mut().take()) {
        hook();
    }
}

fn set_before_recheck(hook: impl FnOnce() + 'static) {
    BEFORE_RECHECK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
}

/// The files set aside beside `path`, with their content.
fn backups(path: &Path) -> Vec<String> {
    let mut kept: Vec<String> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("mixes.json.bad.")
        })
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    kept.sort();
    kept
}

fn genre(name: &str) -> Ingredient {
    Ingredient::Genre { name: name.into() }
}

fn mix(name: &str) -> Mix {
    Mix::new(name, MixSource::Local, vec![genre("Prog Rock")]).unwrap()
}

fn names(mixes: &[Mix]) -> Vec<&str> {
    mixes.iter().map(|m| m.name.as_str()).collect()
}

fn mixes_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("coda").join("mixes.json")
}

#[test]
fn add_update_and_delete_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = mixes_path(&dir);
    assert_eq!(load(&path), (Vec::new(), LoadOutcome::Missing));

    let a = mix("A");
    let b = Mix::new(
        "B",
        MixSource::Navidrome {
            server: "00c36aa1074c465c".into(),
        },
        vec![
            genre("Rock"),
            Ingredient::Artist {
                name: "Yes".into(),
                id: Some("ar-1".into()),
            },
            Ingredient::Album {
                artist: "Yes".into(),
                name: "Fragile".into(),
                id: None,
            },
        ],
    )
    .unwrap();
    assert_eq!(names(&add(&path, a.clone()).unwrap()), ["A"]);
    assert_eq!(names(&add(&path, b.clone()).unwrap()), ["A", "B"]);
    assert_eq!(
        load(&path),
        (
            vec![a.clone(), b.clone()],
            LoadOutcome::Loaded { skipped: 0 }
        )
    );
    assert!(matches!(add(&path, a.clone()), Err(StoreError::Exists)));

    let renamed = Mix {
        name: "A2".into(),
        ..a.clone()
    };
    assert_eq!(
        names(&update(&path, renamed).unwrap()),
        ["A2", "B"],
        "in place"
    );
    assert!(matches!(update(&path, mix("C")), Err(StoreError::NotFound)));
    let blank = Mix {
        name: " ".into(),
        ..a.clone()
    };
    assert!(matches!(
        update(&path, blank),
        Err(StoreError::Invalid(MixError::NoName))
    ));

    assert_eq!(names(&delete(&path, &a.id).unwrap()), ["B"]);
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(names(&delete(&path, &a.id).unwrap()), ["B"], "already gone");
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );

    // Versioned, readable JSON; no temp files left behind.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"version\": 1"), "{text}");
    let mut files: Vec<String> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, [".mixes.json.lock", "mixes.json"]);
}

#[test]
fn every_change_rereads_the_file_so_two_writers_keep_both() {
    let dir = tempfile::tempdir().unwrap();
    let path = mixes_path(&dir);
    // One app's list is stale when the other adds: each change still
    // applies to what is on disk.
    let stale = load(&path).0;
    add(&path, mix("From the GUI")).unwrap();
    assert!(stale.is_empty());
    let after = add(&path, mix("From the TUI")).unwrap();
    assert_eq!(names(&after), ["From the GUI", "From the TUI"]);

    // Writers at the same time: the lock serialises them.
    let threads: Vec<_> = (0..4)
        .map(|t| {
            let path = path.clone();
            std::thread::spawn(move || {
                for n in 0..10 {
                    add(&path, mix(&format!("{t}-{n}"))).unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let (mixes, outcome) = load(&path);
    assert_eq!(outcome, LoadOutcome::Loaded { skipped: 0 });
    assert_eq!(mixes.len(), 42, "every change kept");
}

#[test]
fn a_bad_file_is_kept_aside_and_an_empty_list_used() {
    let dir = tempfile::tempdir().unwrap();
    let path = mixes_path(&dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    for bad in [
        "not json",
        r#"{"mixes":[]}"#,
        r#"{"version":0,"mixes":[]}"#,
        r#"{"version":1}"#,
    ] {
        std::fs::write(&path, bad).unwrap();
        let (mixes, outcome) = load(&path);
        assert!(mixes.is_empty());
        let LoadOutcome::Rejected {
            reason,
            kept_as: Some(kept),
        } = outcome
        else {
            panic!("{bad}: {outcome:?}");
        };
        assert!(!reason.contains("json\""), "{reason}");
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), bad);
        std::fs::remove_file(kept).unwrap();
    }
    // A change after a bad file sets it aside and saves the change.
    std::fs::write(&path, "{").unwrap();
    assert_eq!(names(&add(&path, mix("New")).unwrap()), ["New"]);
    let kept = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("mixes.json.bad.")
        })
        .count();
    assert_eq!(kept, 1);
    // Too large: rejected unread.
    std::fs::write(&path, vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();
    assert!(matches!(load(&path).1, LoadOutcome::Rejected { .. }));
    // Not a regular file.
    let dir_path = dir.path().join("dir.json");
    std::fs::create_dir(&dir_path).unwrap();
    assert!(matches!(
        load(&dir_path).1,
        LoadOutcome::Rejected {
            kept_as: Some(_),
            ..
        }
    ));
}

#[test]
fn a_newer_file_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = mixes_path(&dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let newer = r#"{"version":2,"mixes":[{"whatever":true}]}"#;
    std::fs::write(&path, newer).unwrap();
    assert_eq!(load(&path), (Vec::new(), LoadOutcome::Newer { version: 2 }));
    assert!(matches!(add(&path, mix("A")), Err(StoreError::Newer)));
    assert!(matches!(
        delete(&path, &mix("A").id),
        Err(StoreError::Newer)
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
}

#[test]
fn unusable_mixes_and_ingredients_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let path = mixes_path(&dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = r#"{"version":1,"mixes":[
        {"id":"a","name":"Good","source":{"kind":"local"},
         "ingredients":[{"kind":"genre","name":"Rock"},{"kind":"decade","name":"70s"},
                        {"kind":"artist","name":"Yes","id":"ar-1"},{"kind":"album","name":"No artist"}]},
        {"id":"a","name":"Same id","source":{"kind":"local"},"ingredients":[{"kind":"genre","name":"Pop"}]},
        {"id":"bad id","name":"Bad id","source":{"kind":"local"},"ingredients":[{"kind":"genre","name":"Pop"}]},
        {"id":"b","name":"Bad server","source":{"kind":"navidrome","server":"https://x"},"ingredients":[{"kind":"genre","name":"Pop"}]},
        {"id":"c","name":"Nothing usable","source":{"kind":"local"},"ingredients":[{"kind":"genre","name":" "}]},
        {"id":"d","name":"Other source","source":{"kind":"cloud"},"ingredients":[{"kind":"genre","name":"Pop"}]},
        "not a mix",
        {"id":"e","name":"Remote","source":{"kind":"navidrome","server":"00c36aa1074c465c"},
         "ingredients":[{"kind":"genre","name":"Rock "},{"kind":"album","artist":"Yes","name":"Fragile","id":"al-1"}]}
    ]}"#;
    std::fs::write(&path, text).unwrap();
    let (mixes, outcome) = load(&path);
    assert_eq!(outcome, LoadOutcome::Loaded { skipped: 6 });
    assert_eq!(names(&mixes), ["Good", "Remote"]);
    assert_eq!(
        mixes[0].ingredients,
        [
            genre("Rock"),
            Ingredient::Artist {
                name: "Yes".into(),
                id: None
            }
        ],
        "unknown kinds dropped; a local mix keeps no ids"
    );
    assert_eq!(
        mixes[1].ingredients,
        [
            genre("Rock "),
            Ingredient::Album {
                artist: "Yes".into(),
                name: "Fragile".into(),
                id: Some("al-1".into())
            }
        ]
    );
    // Ingredients past the cap are dropped, not the mix.
    let many: Vec<String> = (0..MAX_INGREDIENTS + 5)
        .map(|i| format!(r#"{{"kind":"genre","name":"g{i}"}}"#))
        .collect();
    let text = format!(
        r#"{{"version":1,"mixes":[{{"id":"a","name":"Many","source":{{"kind":"local"}},"ingredients":[{}]}}]}}"#,
        many.join(",")
    );
    std::fs::write(&path, text).unwrap();
    assert_eq!(load(&path).0[0].ingredients.len(), MAX_INGREDIENTS);
}

#[test]
fn at_most_500_mixes_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = mixes_path(&dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let all: Vec<Mix> = (0..MAX_MIXES + 3).map(|n| mix(&format!("M{n}"))).collect();
    std::fs::write(&path, encode(&all).unwrap()).unwrap();
    let (mixes, outcome) = load(&path);
    assert_eq!(mixes.len(), MAX_MIXES);
    assert_eq!(outcome, LoadOutcome::Loaded { skipped: 3 });
    assert!(matches!(add(&path, mix("One more")), Err(StoreError::Full)));
    // Deleting still works (and drops the ones over the cap).
    assert_eq!(delete(&path, &mixes[0].id).unwrap().len(), MAX_MIXES - 1);
    assert_eq!(add(&path, mix("Room")).unwrap().len(), MAX_MIXES);
}

#[test]
fn a_reader_never_sets_aside_a_file_a_writer_just_saved() {
    let dir = tempfile::tempdir().unwrap();
    let path = mixes_path(&dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "not json").unwrap();
    // This reader has read the bad file; before it sets it aside, another
    // app's add sets the bad file aside itself and saves its mix.
    let writer_path = path.clone();
    set_before_recheck(move || {
        std::thread::spawn(move || add(&writer_path, mix("Saved")).unwrap())
            .join()
            .unwrap();
    });
    let (mixes, outcome) = load(&path);
    assert_eq!(outcome, LoadOutcome::Loaded { skipped: 0 });
    assert_eq!(names(&mixes), ["Saved"]);
    assert_eq!(names(&load(&path).0), ["Saved"], "the canonical file");
    assert_eq!(backups(&path), ["not json"], "the bad file, kept once");
    assert_eq!(
        names(&add(&path, mix("Later")).unwrap()),
        ["Saved", "Later"]
    );
    assert_eq!(backups(&path), ["not json"]);
}

#[test]
fn two_readers_of_a_bad_file_set_it_aside_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = mixes_path(&dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{").unwrap();
    let other = path.clone();
    set_before_recheck(move || {
        let outcome = std::thread::spawn(move || load(&other).1).join().unwrap();
        assert!(matches!(
            outcome,
            LoadOutcome::Rejected {
                kept_as: Some(_),
                ..
            }
        ));
    });
    assert_eq!(load(&path), (Vec::new(), LoadOutcome::Missing));
    assert_eq!(backups(&path), ["{"]);
}

#[test]
fn a_file_near_the_cap_can_still_shrink() {
    // A compact file that loads but is over the cap pretty-printed (as
    // Coda writes it): a change is then written compact. Growing past the
    // cap is refused; deleting works, and the result still loads.
    let dir = tempfile::tempdir().unwrap();
    let path = mixes_path(&dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let big = |n: usize| {
        let genres = (0..MAX_INGREDIENTS)
            .map(|g| genre(&format!("{g:03}{}", "x".repeat(400))))
            .collect();
        Mix::new(format!("M{n}"), MixSource::Local, genres).unwrap()
    };
    let one = encode_compact(&[big(0)]).unwrap().len() as u64;
    let mut mixes: Vec<Mix> = (0..(MAX_FILE_BYTES / one) as usize - 1).map(big).collect();
    while encode_compact(&mixes).unwrap().len() as u64 <= MAX_FILE_BYTES {
        mixes.push(big(mixes.len()));
    }
    mixes.pop();
    assert!(encode(&mixes).unwrap().len() as u64 > MAX_FILE_BYTES);
    std::fs::write(&path, encode_compact(&mixes).unwrap()).unwrap();
    let (loaded, outcome) = load(&path);
    assert_eq!(outcome, LoadOutcome::Loaded { skipped: 0 });
    assert_eq!(loaded.len(), mixes.len());
    assert!(matches!(add(&path, big(999)), Err(StoreError::TooLarge)));
    let after = delete(&path, &mixes[0].id).unwrap();
    assert_eq!(after.len(), mixes.len() - 1);
    let (loaded, outcome) = load(&path);
    assert_eq!(outcome, LoadOutcome::Loaded { skipped: 0 });
    assert_eq!(loaded.len(), mixes.len() - 1);
    assert_eq!(backups(&path), Vec::<String>::new());
}
