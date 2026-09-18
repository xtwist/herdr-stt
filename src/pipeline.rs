//! Shared post-processing for microphone recordings and audio files.
use crate::config::Config;
use crate::corrector;
use crate::intent::{Intent, apply_replacements, parse};
use crate::protocol::{HerdrContext, Transcript};
use crate::vocabulary::{ProjectVocabulary, apply_aliases};

pub async fn build_transcript(
    config: &Config,
    worker_transcript: crate::worker::WorkerTranscript,
    info: crate::worker::WorkerInfo,
    context: HerdrContext,
    vocabulary: ProjectVocabulary,
    routed: bool,
    recognition: &crate::context::ResolvedContext,
) -> Transcript {
    let raw_text = worker_transcript.text.trim().to_string();
    let deterministic_text = apply_aliases(
        &apply_replacements(&raw_text, &config.normalization.replacements),
        &vocabulary,
    );
    let allow_correction = !routed || matches!(parse(&deterministic_text), Intent::Insert(_));
    let corrected = corrector::correct(
        &deterministic_text,
        &worker_transcript,
        &vocabulary,
        &config.correction,
        allow_correction,
        routed,
        &recognition.text,
    )
    .await;
    Transcript {
        context_profile: recognition.profile.clone(),
        context_truncated: recognition.truncated || worker_transcript.context_truncated,
        raw_text,
        deterministic_text,
        text: corrected.text,
        language: worker_transcript.language,
        language_probability: worker_transcript.language_probability,
        duration_seconds: worker_transcript.duration_seconds,
        duration_after_vad_seconds: worker_transcript.duration_after_vad_seconds,
        model: info.model,
        device: info.device,
        compute_type: info.compute_type,
        asr_quality: corrected.quality,
        correction: corrected.report,
        context,
    }
}
