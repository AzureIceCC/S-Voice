//! Ollama client for the post-STT polish step.

use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use thiserror::Error;

const OLLAMA_BASE: &str = "http://localhost:11434";

#[derive(Debug, Error)]
pub enum PolishError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("ollama error: {0}")]
    Ollama(String),
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    response: String,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    error: Option<String>,
}

const DEFAULT_PROMPT: &str = r#"你是语音输入润色助手。用户给你一段口述的原始转写文本。

你的任务（**只做最小整理，最大限度保留口语风格**）：
1. 保留原文中除纯填充词（"嗯"、"啊"、"呃"、"um"、"uh"）外的所有内容
2. 保留单次出现的连接词和停顿词（"然后"、"那个"、"就是说"、"就是"），只去除连续重复（"那个那个"、"就是就是"）
3. 完整保留口语特征：句末语气词（"吧"、"呢"、"啊"、"嘛"）、停顿、连接词
4. 根据语义添加合适的标点（句号、逗号、问号、感叹号、引号）
5. 检查错别字并纠正（同音字错误）
6. 保留用户原本的语法习惯（"我想确认一下事情"保持原文，不要简化为"我想确认事情"）
7. 输出内容严格限定在用户说过的话之内
8. 只输出整理后的纯文本

核心原则：用户说话的语感最重要。如果原话是口语化、半通顺的，就保留这种状态。"#;

pub async fn polish(text: &str, model: &str, keep_alive: &str) -> Result<String, PolishError> {
    polish_with_prompt(text, model, keep_alive, DEFAULT_PROMPT).await
}

pub async fn polish_with_prompt(
    text: &str,
    model: &str,
    keep_alive: &str,
    system_prompt: &str,
) -> Result<String, PolishError> {
    let body = json!({
        "model": model,
        "prompt": text,
        "stream": false,
        "keep_alive": keep_alive,
        "system": system_prompt,
        // Disable thinking mode for polish — qwen3 / DeepSeek-R1 etc. otherwise
        // spend 60s+ reasoning before emitting the actual rewrite. Requires
        // Ollama >= 0.5.0; silently ignored on older versions.
        "think": false,
        "options": {
            "temperature": 0.2,
            // Polish output is typically 50-200 tokens (a single sentence
            // or short paragraph). 1024 was 5-8x more than needed and just
            // reserved extra generation headroom that the model rarely
            // touched. 512 keeps the upper bound generous while letting
            // the model stop earlier when done.
            "num_predict": 512,
            // Cap the KV cache. Ollama defaults to the model's full
            // context length (262144 for qwen3.5), which on Mac unified
            // memory means the first polish after an idle period pays
            // ~30s of model-load cost even when keep_alive says it
            // shouldn't. Limiting to 2048 trims the cache and noticeably
            // speeds up the first call after a reload. Bump if the
            // system prompt or STT output ever exceeds this.
            "num_ctx": 2048
        }
    });

    let t0 = std::time::Instant::now();
    let res = reqwest::Client::new()
        .post(format!("{OLLAMA_BASE}/api/generate"))
        .json(&body)
        // Hard cap on the request itself; Ollama can hang indefinitely
        // (slow model load + thinking) without this.
        .timeout(Duration::from_secs(30))
        .send()
        .await?;
    let t_send_done = t0.elapsed();
    let http_status = res.status();
    let parsed: OllamaResponse = res.error_for_status()?.json().await?;
    let t_full_done = t0.elapsed();

    // Surface the gap between "Ollama finished generating" and "we got the
    // bytes back" — a big difference means the bottleneck is JSON
    // serialization or loopback, not the model itself. The first duration
    // is dominated by generation; the second adds response transfer.
    tracing::info!(
        "polish timing: send_done={:?} full_done={:?} status={} resp_len={}",
        t_send_done,
        t_full_done,
        http_status,
        parsed.response.len()
    );
    if let Some(err) = parsed.error {
        return Err(PolishError::Ollama(err));
    }
    if !parsed.done {
        return Err(PolishError::Ollama("response incomplete".into()));
    }
    Ok(parsed.response.trim().to_string())
}
