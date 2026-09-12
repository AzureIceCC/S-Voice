//! Apple Speech bridge. The Swift helper is embedded in the Rust binary and
//! extracted into the app cache on first use, so release bundles remain
//! self-contained. Apple failures are returned to the pipeline, which decides
//! whether to fall back to the existing local Whisper backend.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use thiserror::Error;

const HELPER_BYTES: &[u8] = include_bytes!(env!("APPLE_SPEECH_HELPER_PATH"));
const HELPER_TIMEOUT: Duration = Duration::from_secs(50);
static HELPER_PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();

#[derive(Debug, Error)]
pub enum AppleSpeechError {
    #[error("helper setup failed: {0}")]
    Setup(String),
    #[error("helper failed: {0}")]
    Helper(String),
    #[error("helper response was invalid: {0}")]
    InvalidResponse(String),
    #[error("Apple Speech request timed out")]
    Timeout,
}

#[derive(Deserialize)]
struct HelperResponse {
    ok: bool,
    #[serde(default)]
    text: String,
    #[serde(default)]
    error: String,
}

fn install_helper() -> Result<PathBuf, String> {
    let dir = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("com.s-voice.app");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("apple-speech-helper");
    let needs_write = std::fs::read(&path).map_or(true, |data| data != HELPER_BYTES);
    if needs_write {
        let temporary = dir.join("apple-speech-helper.tmp");
        std::fs::write(&temporary, HELPER_BYTES).map_err(|e| e.to_string())?;
        set_executable(&temporary).map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions)
}

fn helper_path() -> Result<PathBuf, AppleSpeechError> {
    HELPER_PATH
        .get_or_init(install_helper)
        .clone()
        .map_err(AppleSpeechError::Setup)
}

pub async fn transcribe(wav: Vec<u8>, language: &str) -> Result<String, AppleSpeechError> {
    let locale = apple_locale(language);
    let wav_path = temporary_wav_path();
    std::fs::write(&wav_path, wav).map_err(|e| AppleSpeechError::Setup(e.to_string()))?;
    let helper = helper_path()?;
    let output_task = tokio::task::spawn_blocking({
        let wav_path = wav_path.clone();
        move || {
            std::process::Command::new(helper)
                .args(["transcribe"])
                .arg(&wav_path)
                .arg(locale)
                .output()
        }
    });
    let output = match tokio::time::timeout(HELPER_TIMEOUT, output_task).await {
        Ok(Ok(Ok(output))) => output,
        Ok(Ok(Err(e))) => {
            let _ = std::fs::remove_file(&wav_path);
            return Err(AppleSpeechError::Helper(e.to_string()));
        }
        Ok(Err(e)) => {
            let _ = std::fs::remove_file(&wav_path);
            return Err(AppleSpeechError::Helper(e.to_string()));
        }
        Err(_) => {
            let _ = std::fs::remove_file(&wav_path);
            return Err(AppleSpeechError::Timeout);
        }
    };
    let _ = std::fs::remove_file(&wav_path);
    let response: HelperResponse = serde_json::from_slice(&output.stdout).map_err(|e| {
        AppleSpeechError::InvalidResponse(format!(
            "{e}; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        ))
    })?;
    if output.status.success() && response.ok {
        Ok(response.text)
    } else {
        Err(AppleSpeechError::Helper(if response.error.is_empty() {
            String::from_utf8_lossy(&output.stderr).into_owned()
        } else {
            response.error
        }))
    }
}

fn apple_locale(language: &str) -> &'static str {
    match language {
        "en" => "en-US",
        "ja" => "ja-JP",
        _ => "zh-CN",
    }
}

fn temporary_wav_path() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "svoice-apple-{}-{}.wav",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_supported_language_hints_to_apple_locales() {
        assert_eq!(apple_locale("zh"), "zh-CN");
        assert_eq!(apple_locale("auto"), "zh-CN");
        assert_eq!(apple_locale("en"), "en-US");
        assert_eq!(apple_locale("ja"), "ja-JP");
    }
}
