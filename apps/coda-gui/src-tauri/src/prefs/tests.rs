use super::commands::save_prefs;
use super::*;

fn accent(hex: &str) -> Accent {
    Accent::Color(Rgba::parse_rgb(hex).unwrap())
}

#[test]
fn a_symlinked_file_stays_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("dotfiles").join("settings.toml");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "lyricsIndex = 3\n").unwrap();
    let link = dir.path().join("settings.toml");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let (_, bad) = load(Some(&link));
    assert!(bad);
    let prefs = Prefs {
        accent: accent("#8ea9ff"),
        ..Prefs::default()
    };
    save(&link, &prefs, bad).unwrap();
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(load(Some(&target)).0, prefs);
    // The invalid content was kept aside.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("settings.toml.bad")).unwrap(),
        "lyricsIndex = 3\n"
    );
}

#[test]
fn round_trips_through_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("coda-app").join("settings.toml");
    let prefs = Prefs {
        accent: accent("#c99cff"),
        source: Some(SourceKind::Local),
        lyrics_index: false,
    };
    save(&path, &prefs, false).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("accent = \"#c99cff\""), "{text}");
    assert_eq!(load(Some(&path)), (prefs, false));
    // The house accent is saved by name.
    save(&path, &Prefs::default(), false).unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("accent = \"house\"")
    );
    assert_eq!(load(Some(&path)), (Prefs::default(), false));
}

/// 1.x's file: its theme keys are ignored, the source and the lyrics
/// switch stay, and the next write drops the theme keys (ADR 0009).
#[test]
fn a_1x_file_with_theme_keys_keeps_the_source_and_lyrics_switch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    std::fs::write(
        &path,
        "# Coda (desktop app) preferences. Written by the app.\n\
         mode = \"ghostty\"\n\
         theme = \"Kanso Zen\"\n\
         lightTheme = \"Coda Light\"\n\
         darkTheme = \"Catppuccin Mocha\"\n\
         visualizer = \"palette\"\n\
         source = \"navidrome\"\n\
         lyricsIndex = false\n\
         \n\
         [accents]\n\
         \"Kanso Zen\" = \"#8ba4b0\"\n",
    )
    .unwrap();
    let (prefs, bad) = load(Some(&path));
    assert!(!bad);
    assert_eq!(
        prefs,
        Prefs {
            accent: Accent::House,
            source: Some(SourceKind::Navidrome),
            lyrics_index: false,
        }
    );
    let store = PrefsStore::new(Some(path.clone()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime
        .block_on(save_prefs(&store, |p| p.accent = accent("#7fd8c4")))
        .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    for gone in ["mode", "theme", "Theme", "visualizer", "accents"] {
        assert!(!text.contains(gone), "{gone} in {text}");
    }
    assert_eq!(
        load(Some(&path)).0,
        Prefs {
            accent: accent("#7fd8c4"),
            source: Some(SourceKind::Navidrome),
            lyrics_index: false,
        }
    );
}

#[test]
fn the_source_is_optional_and_checked() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "source = \"navidrome\"\n").unwrap();
    assert_eq!(load(Some(&path)).0.source, Some(SourceKind::Navidrome));
    // Unset: not written at all.
    save(&path, &Prefs::default(), false).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("source"));
    std::fs::write(&path, "source = \"web\"\n").unwrap();
    assert_eq!(load(Some(&path)), (Prefs::default(), true));
}

#[test]
fn missing_or_invalid_files_mean_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    assert_eq!(load(Some(&path)), (Prefs::default(), false));
    assert_eq!(load(None), (Prefs::default(), false));
    std::fs::write(&path, "lyricsIndex = \"yes\"\n").unwrap();
    assert_eq!(load(Some(&path)), (Prefs::default(), true));
    // The bad file is kept aside on the next save.
    save(&path, &Prefs::default(), true).unwrap();
    assert!(dir.path().join("settings.toml.bad").exists());
    assert_eq!(load(Some(&path)), (Prefs::default(), false));
}

#[test]
fn a_bad_accent_falls_back_to_the_house_accent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    for bad in ["red", "#fff", "#11223344", "music", ""] {
        std::fs::write(&path, format!("accent = \"{bad}\"\nlyricsIndex = false\n")).unwrap();
        let (prefs, invalid) = load(Some(&path));
        assert!(!invalid, "{bad}");
        assert_eq!(prefs.accent, Accent::House, "{bad}");
        assert!(!prefs.lyrics_index, "{bad}");
    }
    std::fs::write(&path, "accent = \"#FF7D8E\"\n").unwrap();
    assert_eq!(load(Some(&path)).0.accent, accent("#ff7d8e"));
}

#[test]
fn the_house_colour_typed_as_hex_is_the_house_accent() {
    assert_eq!(Accent::parse("#F0A83A"), Some(Accent::House));
    assert_eq!(Accent::parse("f0a83a"), Some(Accent::House));
    // Saved by name, so it follows the house accent (and N3's default).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let store = PrefsStore::new(Some(path.clone()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime
        .block_on(save_prefs(&store, |p| p.accent = accent("#ff7d8e")))
        .unwrap();
    let typed = Accent::parse("#f0a83a").unwrap();
    runtime
        .block_on(save_prefs(&store, |p| p.accent = typed))
        .unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("accent = \"house\"")
    );
    assert_eq!(PrefsDto::new(&store.current()).accent, "house");
    // Also when a file has it as hex.
    std::fs::write(&path, "accent = \"#f0a83a\"\n").unwrap();
    assert_eq!(load(Some(&path)).0.accent, Accent::House);
}

#[test]
fn accents_parse_strictly() {
    assert_eq!(Accent::parse("house"), Some(Accent::House));
    assert_eq!(Accent::parse(" #8EA9FF "), Some(accent("#8ea9ff")));
    assert_eq!(Accent::parse("8ea9ff"), Some(accent("#8ea9ff")));
    for bad in ["House", "#8ea9f", "#8ea9ffcc", "periwinkle", "rgb(1,2,3)"] {
        assert_eq!(Accent::parse(bad), None, "{bad}");
    }
}

#[test]
fn the_dto_carries_the_colour_its_ink_and_contrast() {
    let house = PrefsDto::new(&Prefs::default());
    assert_eq!(house.accent, "house");
    assert_eq!(house.accent_color, "#f0a83a");
    assert_eq!(house.accent_text, "#000000");
    assert!(house.accent_contrast > 9.0);
    assert_eq!(house.swatches.len(), 6);
    let dark = PrefsDto::new(&Prefs {
        accent: accent("#5a3a8f"),
        ..Prefs::default()
    });
    assert_eq!(dark.accent, "#5a3a8f");
    assert_eq!(dark.accent_text, "#ffffff");
    assert!(dark.accent_contrast < 3.0, "{}", dark.accent_contrast);
    let json = serde_json::to_value(&house).unwrap();
    for key in [
        "accent",
        "accentColor",
        "accentText",
        "accentContrast",
        "swatches",
        "source",
        "lyricsIndex",
    ] {
        assert!(json.get(key).is_some(), "{key}");
    }
}

#[test]
fn the_init_script_sets_only_a_chosen_accent() {
    assert_eq!(init_script(&Prefs::default()), None);
    let script = init_script(&Prefs {
        accent: accent("#ff7d8e"),
        source: Some(SourceKind::Local),
        lyrics_index: true,
    })
    .unwrap();
    assert!(script.contains("\"accent\":\"#ff7d8e\""), "{script}");
    assert!(script.contains("\"accentText\":\"#000000\""), "{script}");
    assert!(script.contains("--accent-text"));
    assert!(!script.contains("local"), "{script}");
}

#[test]
fn the_store_loads_at_startup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "accent = \"#e9e3d2\"\n").unwrap();
    let store = PrefsStore::new(Some(path));
    assert_eq!(store.current().accent, accent("#e9e3d2"));
    assert!(store.init_script().unwrap().contains("#e9e3d2"));
    assert_eq!(PrefsStore::new(None).init_script(), None);
}

#[test]
fn saving_without_a_change_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let store = PrefsStore::new(Some(path.clone()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(save_prefs(&store, |_| {})).unwrap();
    assert!(!path.exists());
    runtime
        .block_on(save_prefs(&store, |p| p.source = Some(SourceKind::Local)))
        .unwrap();
    assert_eq!(load(Some(&path)).0.source, Some(SourceKind::Local));
    // No config folder: an error to show, not a panic.
    let nowhere = PrefsStore::new(None);
    assert!(
        runtime
            .block_on(save_prefs(&nowhere, |p| p.lyrics_index = false))
            .is_err()
    );
}

#[test]
fn the_config_folder_follows_xdg_then_home() {
    let home = Some(OsString::from("/home/a"));
    assert_eq!(
        config_dir(home.clone(), Some("/x/config".into())),
        Some(PathBuf::from("/x/config"))
    );
    // A relative XDG_CONFIG_HOME is ignored (the spec).
    assert_eq!(
        config_dir(home.clone(), Some("rel".into())),
        Some(PathBuf::from("/home/a/.config"))
    );
    assert_eq!(config_dir(None, None), None);
    assert_eq!(
        path(Some(Path::new("/c"))),
        Some(PathBuf::from("/c/coda-app/settings.toml"))
    );
}
