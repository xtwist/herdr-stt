use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const API_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub request_id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub version: u32,
    pub request_id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
}

impl Response {
    pub fn success(request_id: impl Into<String>, result: impl Serialize) -> Self {
        Self {
            version: API_VERSION,
            request_id: request_id.into(),
            ok: true,
            result: Some(serde_json::to_value(result).unwrap_or(Value::Null)),
            error: None,
        }
    }

    pub fn failure(
        request_id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            version: API_VERSION,
            request_id: request_id.into(),
            ok: false,
            result: None,
            error: Some(ApiError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HerdrContext {
    pub socket_path: Option<String>,
    pub herdr_bin_path: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_label: Option<String>,
    pub tab_id: Option<String>,
    pub pane_id: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartParams {
    #[serde(default)]
    pub recognition: crate::context::RecognitionHints,
    #[serde(default)]
    pub context: HerdrContext,
    #[serde(default)]
    pub hotwords: Vec<String>,
    #[serde(default)]
    pub vocabulary: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    #[serde(default)]
    pub context_profile: Option<String>,
    #[serde(default)]
    pub context_truncated: bool,
    pub raw_text: String,
    pub deterministic_text: String,
    pub text: String,
    pub language: Option<String>,
    pub language_probability: Option<f64>,
    pub duration_seconds: f64,
    pub duration_after_vad_seconds: Option<f64>,
    pub model: String,
    pub device: String,
    pub compute_type: String,
    #[serde(default)]
    pub asr_quality: AsrQuality,
    #[serde(default)]
    pub correction: CorrectionReport,
    #[serde(default)]
    pub context: HerdrContext,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AsrQuality {
    pub min_segment_avg_logprob: Option<f64>,
    pub min_word_probability: Option<f64>,
    pub low_confidence_word_ratio: Option<f64>,
    pub word_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CorrectionReport {
    pub status: String,
    pub backend: Option<String>,
    pub trigger_reasons: Vec<String>,
    pub edits_applied: usize,
    pub edits_rejected: usize,
    pub latency_ms: Option<u64>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStatus {
    Active,
    Finishing,
    Recorded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResult {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<CaptureStatus>,
    pub model: String,
    pub device: String,
    pub compute_type: String,
    pub has_last_transcript: bool,
    pub correction_enabled: bool,
    pub correction_backend: Option<String>,
}

#[cfg(test)]
impl Transcript {
    pub(crate) fn default_for_test(text: &str) -> Self {
        Self {
            context_profile: None,
            context_truncated: false,
            raw_text: text.into(),
            deterministic_text: text.into(),
            text: text.into(),
            language: None,
            language_probability: None,
            duration_seconds: 0.0,
            duration_after_vad_seconds: None,
            model: "test".into(),
            device: "cpu".into(),
            compute_type: "int8".into(),
            asr_quality: AsrQuality::default(),
            correction: CorrectionReport::default(),
            context: HerdrContext::default(),
        }
    }
}
