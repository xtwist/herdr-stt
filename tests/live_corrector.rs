use herdr_stt::{
    config::{CorrectionBackend, CorrectionConfig, CorrectionTrigger},
    corrector,
    vocabulary::ProjectVocabulary,
    worker::WorkerTranscript,
};

#[tokio::test]
#[ignore = "requires HERDR_STT_TEST_QWEN_ENDPOINT pointing to a local Qwen server"]
async fn local_qwen_accepts_context_and_preserves_protected_text() {
    let mut config = CorrectionConfig {
        enabled: true,
        backend: CorrectionBackend::Openai,
        trigger: CorrectionTrigger::Always,
        endpoint: std::env::var("HERDR_STT_TEST_QWEN_ENDPOINT").unwrap(),
        timeout_seconds: 90,
        ..CorrectionConfig::default()
    };
    let transcript = WorkerTranscript {
        context_truncated: false,
        text: "проверь юзернейм парсер, не меняй порт 8080".into(),
        language: Some("ru".into()),
        language_probability: Some(1.0),
        duration_seconds: 5.0,
        duration_after_vad_seconds: Some(5.0),
        segments: Vec::new(),
    };
    for backend in [CorrectionBackend::Openai, CorrectionBackend::Managed] {
        config.backend = backend;
        let result = corrector::correct(
            &transcript.text,
            &transcript,
            &ProjectVocabulary::from_terms(vec!["UsernameParser".into()]),
            &config,
            true,
            true,
            "UsernameParser — обработчик имени пользователя в проекте на Rust.",
        )
        .await;
        assert_ne!(result.report.status, "fallback", "{:?}", result.report);
        assert!(result.text.contains("не меняй"));
        assert!(result.text.contains("проверь"));
        assert!(result.text.contains("8080"));
        println!("{}: {}", result.report.status, result.text);
    }
}
