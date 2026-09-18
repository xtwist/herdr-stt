use herdr_stt::config::Config;
use herdr_stt::context::{ContextBinding, Guidance, RecognitionHints, resolve};

#[test]
fn request_overrides_profile_language_and_keeps_specific_text_first() {
    let mut config = Config::default();
    config.context.text = "Global background".into();
    config.context.language = Some("ru".into());
    config.context.default_profile = Some("coding".into());
    config.context.profiles.insert(
        "coding".into(),
        Guidance {
            text: "Profile background".into(),
            language: Some("en".into()),
            hotwords: vec![" Redis ".into(), "MyParser".into()],
            ..Guidance::default()
        },
    );
    let hints = RecognitionHints {
        text: "Current task".into(),
        language: Some("auto".into()),
        hotwords: vec!["REDIS".into()],
        ..RecognitionHints::default()
    };
    let resolved = resolve(&config, None, &hints).unwrap();
    assert_eq!(resolved.profile.as_deref(), Some("coding"));
    assert_eq!(resolved.language, None);
    assert_eq!(
        resolved.text,
        "Current task\n\nProfile background\n\nGlobal background"
    );
    assert!(resolved.initial_prompt.starts_with("Current task"));
    assert_eq!(&resolved.hotwords[..2], ["REDIS", "MyParser"]);
    assert_eq!(
        resolved
            .hotwords
            .iter()
            .filter(|s| s.eq_ignore_ascii_case("redis"))
            .count(),
        1
    );
}

#[test]
fn deepest_directory_binding_wins_and_explicit_profile_overrides_it() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("repo/crate/src")).unwrap();
    std::fs::create_dir_all(temp.path().join("repository")).unwrap();
    let mut config = Config {
        source_path: Some(temp.path().join("config.toml")),
        ..Config::default()
    };
    for name in ["repo", "crate"] {
        config
            .context
            .profiles
            .insert(name.into(), Guidance::default());
    }
    config.context.bindings = vec![
        ContextBinding {
            path: "repo".into(),
            profile: "repo".into(),
        },
        ContextBinding {
            path: "repo/crate".into(),
            profile: "crate".into(),
        },
    ];
    let cwd = temp.path().join("repo/crate/src");
    let resolved = resolve(&config, cwd.to_str(), &RecognitionHints::default()).unwrap();
    assert_eq!(resolved.profile.as_deref(), Some("crate"));
    let explicit = RecognitionHints {
        profile: Some("repo".into()),
        ..RecognitionHints::default()
    };
    assert_eq!(
        resolve(&config, cwd.to_str(), &explicit)
            .unwrap()
            .profile
            .as_deref(),
        Some("repo")
    );
    let near = temp.path().join("repository");
    assert!(
        resolve(&config, near.to_str(), &RecognitionHints::default())
            .unwrap()
            .profile
            .is_none()
    );
}

#[test]
fn context_files_are_relative_to_loaded_config_and_are_bounded_utf8() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        "[context]\nfiles = ['terms.txt']\nmax_file_bytes = 32\n",
    )
    .unwrap();
    std::fs::write(temp.path().join("terms.txt"), "Термины проекта").unwrap();
    let config = Config::load_from(&path).unwrap();
    assert_eq!(
        resolve(&config, None, &RecognitionHints::default())
            .unwrap()
            .text,
        "Термины проекта"
    );
    std::fs::write(temp.path().join("terms.txt"), "a".repeat(33)).unwrap();
    assert!(
        resolve(&config, None, &RecognitionHints::default())
            .unwrap_err()
            .to_string()
            .contains("exceeds")
    );
    std::fs::write(temp.path().join("terms.txt"), [0xff]).unwrap();
    assert!(
        resolve(&config, None, &RecognitionHints::default())
            .unwrap_err()
            .to_string()
            .contains("UTF-8")
    );
}

#[test]
fn truncation_preserves_unicode_and_reports_limits() {
    let mut config = Config::default();
    config.context.max_chars = 4;
    config.context.max_prompt_chars = 3;
    config.context.max_hotwords = 1;
    let hints = RecognitionHints {
        text: "Привет 👋".into(),
        hotwords: vec!["Первый".into(), "Второй".into()],
        ..RecognitionHints::default()
    };
    let resolved = resolve(&config, None, &hints).unwrap();
    assert_eq!(resolved.text, "Прив");
    assert_eq!(resolved.initial_prompt, "При");
    assert_eq!(resolved.hotwords, ["Первый"]);
    assert!(resolved.truncated);
}

#[test]
fn bad_profile_and_bad_configuration_fail_explicitly() {
    let mut config = Config::default();
    let hints = RecognitionHints {
        profile: Some("missing".into()),
        ..RecognitionHints::default()
    };
    assert!(
        resolve(&config, None, &hints)
            .unwrap_err()
            .to_string()
            .contains("unknown context profile")
    );
    config.context.default_profile = Some("missing".into());
    assert!(config.validate().is_err());
    config.context.default_profile = None;
    config.context.max_file_bytes = 0;
    assert!(config.validate().is_err());
    assert!(toml::from_str::<Config>("[context]\nmax_characters = 100").is_err());
}

#[test]
fn old_configuration_needs_no_migration() {
    let config: Config = toml::from_str(
        "[transcription]\ninitial_prompt = 'Existing hint'\nhotwords = ['ExistingTerm']",
    )
    .unwrap();
    config.validate().unwrap();
    let resolved = resolve(&config, None, &RecognitionHints::default()).unwrap();
    assert_eq!(resolved.initial_prompt, "Existing hint");
    assert_eq!(resolved.hotwords, ["ExistingTerm"]);
}
