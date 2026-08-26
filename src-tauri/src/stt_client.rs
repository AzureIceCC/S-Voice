//! HTTP client for the local STT bridge (FastAPI on :18787).
//!
//! Single endpoint: POST /transcribe returns the full text in one shot.
//! v0.2 may add a streaming / VAD-segmented path here.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const STT_BASE: &str = "http://127.0.0.1:18787";

#[derive(Debug, Error)]
pub enum SttError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("bridge unhealthy: {0}")]
    Unhealthy(String),
    #[error("bridge unreachable; is the STT service running? start with scripts/stt/start_stt.sh")]
    Unreachable,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TranscribeResponse {
    pub text: String,
    pub language: String,
    pub duration_sec: Option<f32>,
    pub inference_sec: f32,
    pub model: String,
}

#[derive(Debug, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub model: String,
    pub default_language: String,
}

pub async fn health() -> Result<HealthResponse, SttError> {
    let res = reqwest::get(format!("{STT_BASE}/health"))
        .await
        .map_err(|_| SttError::Unreachable)?;
    let body: HealthResponse = res.error_for_status()?.json().await?;
    if body.status != "ok" {
        return Err(SttError::Unhealthy(body.status));
    }
    Ok(body)
}

/// Format a one-line summary of the STT bridge's `/health` response.
/// Debug-only — used by the `cmd_test_stt` command; release builds strip
/// that command via `#[cfg(debug_assertions)]` and this helper becomes
/// unused.
#[cfg(debug_assertions)]
pub async fn health_string() -> Result<String, SttError> {
    let h = health().await?;
    Ok(format!(
        "STT bridge OK — model={}, lang={}",
        h.model, h.default_language
    ))
}

/// One-shot transcription. The whole audio is sent and the bridge returns
/// the final text in a single response. Latency is dominated by the model
/// forward pass (~4-5s for the v3-turbo Chinese model on M-series).
pub async fn transcribe(wav: Vec<u8>, language: &str) -> Result<TranscribeResponse, SttError> {
    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(wav).file_name("audio.wav"),
        )
        .text("language", language.to_string());

    let res = reqwest::Client::new()
        .post(format!("{STT_BASE}/transcribe"))
        .multipart(form)
        .send()
        .await
        .map_err(|_| SttError::Unreachable)?
        .error_for_status()?;

    let body: TranscribeResponse = res.json().await?;
    Ok(body)
}

/// Resolve the path to the STT start/stop scripts relative to the app's working dir.
/// Tauri runs the binary with cwd = the bundle Resources dir, so we look up via
/// the executable path: `../stt/start_stt.sh` from src-tauri/target/...
pub fn stt_script_path() -> PathBuf {
    script_with_name("start_stt.sh")
}

pub fn stt_stop_script_path() -> PathBuf {
    script_with_name("stop_stt.sh")
}

fn script_with_name(name: &str) -> PathBuf {
    // Look in $STT_HOME, then executable-relative paths, then cwd-relative.
    if let Ok(p) = std::env::var("STT_HOME") {
        let p = PathBuf::from(p).join(name);
        if p.exists() {
            return p;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        // dev: target/debug/s-voice -> src-tauri/target/debug/s-voice
        // -> ../../../../stt/start_stt.sh
        for ancestor in exe.ancestors().take(6) {
            let cand = ancestor.join("stt").join(name);
            if cand.exists() {
                return cand;
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for ancestor in cwd.ancestors().take(6) {
            let cand = ancestor.join("stt").join(name);
            if cand.exists() {
                return cand;
            }
        }
    }
    PathBuf::from(name)
}
