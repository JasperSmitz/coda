use std::path::Path;

use super::*;

struct Fixture {
    _root: tempfile::TempDir,
    dirs: ThemeDirs,
    ghostty_config: PathBuf,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let ghostty = root.path().join("ghostty-themes");
    let custom = root.path().join("custom");
    std::fs::create_dir_all(&ghostty).unwrap();
    std::fs::write(
        ghostty.join("Installed"),
        "background = #101820\nforeground = #e0e0e0\npalette = 3=#ffcc00\n",
    )
    .unwrap();
    Fixture {
        ghostty_config: root.path().join("ghostty-config"),
        dirs: ThemeDirs {
            ghostty: vec![ghostty],
            custom: Some(custom),
        },
        _root: root,
    }
}

fn custom_path(f: &Fixture, name: &str) -> PathBuf {
    f.dirs.custom.as_ref().unwrap().join(name)
}

fn token<'a>(t: &'a ResolvedTheme, key: &str) -> &'a Token {
    t.tokens.iter().find(|t| t.key == key).unwrap()
}

fn edits(pairs: &[(&str, Option<&str>)]) -> Edits {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.map(str::to_owned)))
        .collect()
}

#[test]
fn saved_accent_overrides_apply_and_are_explicit() {
    let f = fixture();
    let mut prefs = Prefs::default();
    prefs.accents.insert("Installed".into(), "#202830".into());
    let entry = sources::find(&f.dirs, "Installed").unwrap();
    let t = resolve(&entry, &prefs, &Edits::new()).unwrap();
    // Too dark on this background, but the user chose it: kept, warned.
    let accent = token(&t, "accent");
    assert_eq!(accent.value, "#202830");
    assert!(accent.overridden && accent.contrast.is_some());
    // Without the override: palette 3.
    let t = resolve(&entry, &Prefs::default(), &Edits::new()).unwrap();
    assert_eq!(token(&t, "accent").value, "#ffcc00");
}

#[test]
fn edits_are_validated() {
    let f = fixture();
    let entry = sources::find(&f.dirs, "Installed").unwrap();
    let prefs = Prefs::default();
    assert!(resolve(&entry, &prefs, &edits(&[("nope", Some("#000000"))])).is_err());
    assert!(resolve(&entry, &prefs, &edits(&[("bg", Some("#00000080"))])).is_err());
    assert!(resolve(&entry, &prefs, &edits(&[("hover", Some("red"))])).is_err());
    let t = resolve(
        &entry,
        &prefs,
        &edits(&[("hover", Some("#ffffff20")), ("palette-3", Some("#00ff00"))]),
    )
    .unwrap();
    assert_eq!(token(&t, "hover").value, "#ffffff20");
    assert_eq!(token(&t, "accent").value, "#00ff00");
}

#[test]
fn modes_pick_their_themes() {
    let f = fixture();
    let configs = vec![f.ghostty_config.clone()];
    let single = Prefs {
        mode: Mode::Single,
        theme: "Installed".into(),
        ..Prefs::default()
    };
    let a = applied(&f.dirs, &configs, &single);
    assert_eq!(
        (a.light.name.as_str(), a.dark.name.as_str()),
        ("Installed", "Installed")
    );
    assert!(a.notes.is_empty());

    let a = applied(&f.dirs, &configs, &Prefs::default());
    assert_eq!(
        (a.light.name.as_str(), a.dark.name.as_str()),
        ("Coda Light", "Coda")
    );

    let ghostty = Prefs {
        mode: Mode::Ghostty,
        ..Prefs::default()
    };
    // No Ghostty config: the defaults, with a note.
    let a = applied(&f.dirs, &configs, &ghostty);
    assert_eq!(a.dark.name, "Coda");
    assert_eq!(a.notes.len(), 1);
    std::fs::write(
        &f.ghostty_config,
        "theme = light:Kanso Pearl,dark:Installed\n",
    )
    .unwrap();
    let a = applied(&f.dirs, &configs, &ghostty);
    assert_eq!(
        (a.light.name.as_str(), a.dark.name.as_str()),
        ("Kanso Pearl", "Installed")
    );
    assert_eq!(a.ghostty.unwrap().dark, "Installed");
}

#[test]
fn missing_themes_fall_back_with_a_note() {
    let f = fixture();
    let prefs = Prefs {
        light_theme: "Gone".into(),
        ..Prefs::default()
    };
    let a = applied(&f.dirs, &[], &prefs);
    assert_eq!(a.light.name, DEFAULT_LIGHT);
    assert_eq!(a.light.source, Source::BuiltIn);
    assert!(a.notes[0].contains("Gone"));
}

#[test]
fn saving_a_custom_theme_writes_a_ghostty_file_and_selects_it() {
    let f = fixture();
    let mut prefs = Prefs {
        mode: Mode::Single,
        ..Prefs::default()
    };
    prefs.accents.insert("Kanso Zen".into(), "#aabbcc".into());
    let e = edits(&[("surface-2", Some("#123456")), ("bg", Some("#000000"))]);
    save_custom(
        &f.dirs,
        &mut prefs,
        "Kanso Zen",
        "Kanso Zen (custom)",
        &e,
        None,
    )
    .unwrap();
    assert_eq!(prefs.theme, "Kanso Zen (custom)");
    let text = std::fs::read_to_string(custom_path(&f, "Kanso Zen (custom)")).unwrap();
    assert!(text.contains("background = #000000"));
    assert!(text.contains("coda-surface-2 = #123456"));
    // The saved accent override is baked into the copy.
    assert!(text.contains("coda-accent = #aabbcc"));
    assert!(text.contains("palette = 4=#8ba4b0"));
    let entry = sources::find(&f.dirs, "Kanso Zen (custom)").unwrap();
    assert_eq!(entry.source, Source::Custom);
    // The built-in one is untouched.
    assert_eq!(
        sources::find(&f.dirs, "Kanso Zen").unwrap().source,
        Source::BuiltIn
    );

    // Editing it again in place; then renaming while saving.
    let e = edits(&[("surface-2", None), ("border", Some("#222222"))]);
    save_custom(
        &f.dirs,
        &mut prefs,
        "Kanso Zen (custom)",
        "Mine",
        &e,
        Some("Kanso Zen (custom)"),
    )
    .unwrap();
    assert!(!custom_path(&f, "Kanso Zen (custom)").exists());
    let text = std::fs::read_to_string(custom_path(&f, "Mine")).unwrap();
    assert!(!text.contains("coda-surface-2"));
    assert!(text.contains("coda-border = #222222"));
    assert_eq!(prefs.theme, "Mine");
}

#[test]
fn custom_names_never_replace_other_themes() {
    let f = fixture();
    let mut prefs = Prefs::default();
    for taken in ["Coda", "Installed", "../x", ""] {
        assert!(
            save_custom(&f.dirs, &mut prefs, "Coda", taken, &Edits::new(), None).is_err(),
            "{taken}"
        );
    }
    save_custom(&f.dirs, &mut prefs, "Coda", "A", &Edits::new(), None).unwrap();
    assert!(save_custom(&f.dirs, &mut prefs, "Coda", "A", &Edits::new(), None).is_err());
    // Only custom themes can be edited in place, renamed or deleted.
    assert!(
        save_custom(
            &f.dirs,
            &mut prefs,
            "Coda",
            "B",
            &Edits::new(),
            Some("Coda")
        )
        .is_err()
    );
    assert!(rename_custom(&f.dirs, &mut prefs, "Installed", "B").is_err());
    assert!(delete_custom(&f.dirs, &mut prefs, "Coda").is_err());
    assert!(delete_custom(&f.dirs, &mut prefs, "../custom/A").is_err());
}

#[test]
fn rename_duplicate_and_delete_custom_themes() {
    let f = fixture();
    let mut prefs = Prefs::default();
    // Saved in system mode: selected for its scheme (dark here).
    save_custom(&f.dirs, &mut prefs, "Installed", "A", &Edits::new(), None).unwrap();
    assert_eq!(prefs.dark_theme, "A");
    assert_eq!(prefs.mode, Mode::System);
    rename_custom(&f.dirs, &mut prefs, "A", "B").unwrap();
    assert_eq!(prefs.dark_theme, "B");
    assert!(custom_path(&f, "B").exists() && !custom_path(&f, "A").exists());
    duplicate(&f.dirs, &prefs, "B", "C").unwrap();
    duplicate(&f.dirs, &prefs, "Catppuccin Latte", "D").unwrap();
    assert!(Path::new(&custom_path(&f, "D")).exists());
    assert!(duplicate(&f.dirs, &prefs, "B", "C").is_err());
    delete_custom(&f.dirs, &mut prefs, "B").unwrap();
    assert!(!custom_path(&f, "B").exists());
    assert_eq!(prefs.dark_theme, DEFAULT_DARK);
}

#[test]
fn init_script_carries_tokens_not_names() {
    let f = fixture();
    let prefs = Prefs {
        mode: Mode::Single,
        theme: "Installed".into(),
        ..Prefs::default()
    };
    let a = applied(&f.dirs, &[], &prefs);
    let script = init_script(&a, &prefs);
    assert!(script.contains("\"bg\":\"#101820\""));
    assert!(script.contains("__CODA_THEME__"));
    assert!(!script.contains("Installed"));
}

/// Every theme installed on this machine (Ghostty's included) resolves,
/// and the guarded tokens pass their contrast checks. Reads the real
/// theme folders, so it is not run by default.
#[test]
#[ignore]
fn installed_themes_resolve_and_pass_the_guard() {
    const GUARDED: [&str; 15] = [
        "text",
        "text-2",
        "text-3",
        "accent",
        "danger",
        "success",
        "np-text",
        "selection-text",
        "cover-1",
        "cover-2",
        "cover-3",
        "cover-4",
        "cover-5",
        "cover-6",
        "bg",
    ];
    let dirs = ThemeDirs::from_process();
    let all = list(&dirs, &Prefs::default());
    println!("{} themes", all.len());
    let mut unguarded = 0;
    for entry in sources::load_all(&dirs).values() {
        let t = resolve(entry, &Prefs::default(), &Edits::new()).unwrap();
        for token in &t.tokens {
            if let Some(w) = &token.contrast {
                // Pathological themes (mid-grey backgrounds) may have no
                // passing colour at all.
                let against = t.tokens.iter().find(|a| a.key == w.against).unwrap();
                let against = color::Rgba::parse(&against.value).unwrap();
                let reachable = color::contrast(color::WHITE, against)
                    .max(color::contrast(color::BLACK, against))
                    >= w.min;
                if GUARDED.contains(&token.key.as_str())
                    && entry.source != Source::Custom
                    && reachable
                {
                    panic!("{}: {} {:?}", entry.name, token.key, w);
                }
                unguarded += 1;
            }
        }
    }
    println!("{unguarded} warnings on unguarded tokens (fg, accent-text, danger-text)");
}

/// `src/styles/tokens.css` holds the built-in Coda and Coda Light tokens
/// as fallbacks: they must match what the themes resolve to.
#[test]
fn tokens_css_defaults_match_the_built_in_themes() {
    let css = include_str!("../../../src/styles/tokens.css");
    let (dark_css, rest) = css
        .split_once("@media (prefers-color-scheme: light)")
        .unwrap();
    let light_css = rest.split("@media (prefers-reduced-motion").next().unwrap();
    for (name, block) in [("Coda", dark_css), ("Coda Light", light_css)] {
        let entry = sources::find(&ThemeDirs::default(), name).unwrap();
        let t = resolve(&entry, &Prefs::default(), &Edits::new()).unwrap();
        for token in &t.tokens {
            let line = format!("--{}: {};", token.key, token.value);
            assert!(block.contains(&line), "{name}: tokens.css lacks `{line}`");
        }
    }
}
