//! HTTP client for the local STT bridge (FastAPI on :18787).
//!
//! Single endpoint: POST /transcribe returns the full text in one shot.
//! v0.2 may add a streaming / VAD-segmented path here.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const STT_BASE: &str = "http://127.0.0.1:18787";
const STT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
// Observed worst case is roughly 5-15s to reload the model plus up to 10s
// inference. Keep enough headroom for a cold request while still guaranteeing
// that the pipeline eventually leaves Processing if MLX wedges.
const STT_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Error)]
pub enum SttError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("bridge unhealthy: {0}")]
    Unhealthy(String),
    #[error("bridge unreachable; is the STT service running? start with scripts/stt/start_stt.sh")]
    Unreachable,
    #[error("STT request timed out after {0:?}")]
    Timeout(Duration),
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(STT_CONNECT_TIMEOUT)
            .timeout(STT_REQUEST_TIMEOUT)
            .build()
            .expect("building the static STT HTTP client should not fail")
    })
}

fn classify_request_error(error: reqwest::Error, timeout: Duration) -> SttError {
    if error.is_timeout() {
        SttError::Timeout(timeout)
    } else if error.is_connect() {
        SttError::Unreachable
    } else {
        SttError::Http(error)
    }
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
    let timeout = Duration::from_secs(3);
    let res = client()
        .get(format!("{STT_BASE}/health"))
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| classify_request_error(e, timeout))?
        .error_for_status()
        .map_err(|e| classify_request_error(e, timeout))?;
    let body: HealthResponse = res
        .json()
        .await
        .map_err(|e| classify_request_error(e, timeout))?;
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
/// Fire-and-forget prewarm: tell the STT bridge to start (re)loading
/// its model in the background. Called from the Rust pipeline the moment
/// the user presses the global hotkey — the model finishes loading
/// while the user is still speaking (typically 5-10s), so the
/// subsequent /transcribe hits a warm model and pays ~0s cold-start.
///
/// We block on the response (which itself blocks until the bridge's
/// background load finishes, see `_load_lock` in stt_server.py) up to
/// 20s. The user's speech window is typically 5-10s, so the model
/// *will* be ready by the time recording ends. A 500ms cap (the
/// earlier "fire and forget" version) made this optimisation useless
/// — the bridge's load would still be in flight when /transcribe
/// fired. The 20s cap protects against a misbehaving bridge; a real
/// STT cold load is 5-15s.
pub async fn prewarm() {
    let started = std::time::Instant::now();
    let res = client()
        .post(format!("{STT_BASE}/prewarm"))
        .timeout(Duration::from_secs(20))
        .send()
        .await;
    match res {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            tracing::info!(
                "STT prewarm done: status={} elapsed={}ms body={}",
                status,
                started.elapsed().as_millis(),
                body
            );
        }
        Err(e) => {
            tracing::warn!(
                "STT prewarm failed (non-fatal, elapsed={}ms): {e}",
                started.elapsed().as_millis()
            );
        }
    }
}

pub async fn transcribe(wav: Vec<u8>, language: &str) -> Result<TranscribeResponse, SttError> {
    transcribe_with_client(client(), STT_BASE, wav, language, STT_REQUEST_TIMEOUT).await
}

async fn transcribe_with_client(
    client: &reqwest::Client,
    base_url: &str,
    wav: Vec<u8>,
    language: &str,
    timeout: Duration,
) -> Result<TranscribeResponse, SttError> {
    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(wav).file_name("audio.wav"),
        )
        .text("language", language.to_string());

    let res = client
        .post(format!("{base_url}/transcribe"))
        .timeout(timeout)
        .multipart(form)
        .send()
        .await
        .map_err(|e| classify_request_error(e, timeout))?
        .error_for_status()
        .map_err(|e| classify_request_error(e, timeout))?;

    let body: TranscribeResponse = res
        .json()
        .await
        .map_err(|e| classify_request_error(e, timeout))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[tokio::test]
    async fn transcribe_times_out_when_server_never_responds() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            std::thread::sleep(Duration::from_millis(250));
        });

        let timeout = Duration::from_millis(50);
        let test_client = reqwest::Client::builder()
            .connect_timeout(timeout)
            .timeout(timeout)
            .build()
            .unwrap();
        let result = transcribe_with_client(
            &test_client,
            &format!("http://{address}"),
            b"not-a-real-wav".to_vec(),
            "auto",
            timeout,
        )
        .await;

        assert!(matches!(result, Err(SttError::Timeout(t)) if t == timeout));
        server.join().unwrap();
    }
}
