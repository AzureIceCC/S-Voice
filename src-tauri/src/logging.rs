//! Tracing setup: stdout + synchronous file log + macOS unified log.
//!
//! v1 design: we write directly to a single `s-voice.log` file (no daily
//! rotation, no BufWriter, no background thread). Each `tracing::info!`
//! call does a single `write()` syscall, so data is on disk before the
//! call returns. This is slightly slower than the async/buffered path
//! tracing-appender uses, but it survives `kill` and `pkill` without
//! losing recent lines. The reload handle lets us flip verbosity at
//! runtime via the settings UI.
//!
//! On macOS we additionally mirror every event to the unified log via
//! `tracing-oslog`. Console.app and `log stream` pick it up with no
//! extra setup. cfg-gated so the layer compiles to nothing on other
//! platforms (S-Voice is currently macOS-only, but keeping the gate
//! means adding a Windows/Linux target later won't drag oslog in).

use std::path::PathBuf;
use std::sync::OnceLock;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, reload, EnvFilter, Registry};

type FilterHandle = reload::Handle<EnvFilter, Registry>;

static FILTER_HANDLE: OnceLock<FilterHandle> = OnceLock::new();

/// Where the log file lives. macOS convention: `~/Library/Logs/S-Voice/`.
pub fn log_dir() -> PathBuf {
    // ~/Library/Logs/S-Voice — Apple-recommended location for app logs.
    // We avoid `dirs::data_local_dir()` because on macOS that resolves to
    // ~/Library/Application Support/Logs/, which is the wrong place.
    let home = dirs::home_dir().unwrap_or_default();
    home.join("Library").join("Logs").join("S-Voice")
}

pub fn current_log_path() -> Option<PathBuf> {
    let dir = log_dir();
    let p = dir.join("s-voice.log");
    if p.exists() {
        Some(p)
    } else {
        std::fs::create_dir_all(&dir).ok()?;
        Some(p)
    }
}

/// Initialize tracing. Idempotent — subsequent calls are no-ops.
/// Returns true if initialization actually happened on this call.
pub fn init(debug_enabled: bool) -> bool {
    if FILTER_HANDLE.get().is_some() {
        return false;
    }
    let dir = log_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("warning: failed to create log dir {dir:?}: {e}");
    }
    let log_file = dir.join("s-voice.log");
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)
    {
        Ok(f) => f,
        Err(e) => {
            eprintln!("warning: failed to open log file {log_file:?}: {e}");
            return false;
        }
    };

    // Build filters. The reload handle lets us flip verbosity at runtime.
    let env_default = if debug_enabled {
        "s_voice_lib=debug"
    } else {
        "s_voice_lib=info"
    };
    let initial_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(env_default));

    let (filter, handle) = reload::Layer::new(initial_filter);

    let stdout_layer: fmt::Layer<_, _, _, _> = fmt::layer()
        .with_writer(std::io::stdout)
        .with_target(true)
        .with_thread_ids(false)
        .with_line_number(false);

    let file_layer: fmt::Layer<_, _, _, _> = fmt::layer()
        .with_writer(file)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(false)
        .with_line_number(false);

    // macOS unified log mirror. Subsystem matches the bundle id so
    // Console.app groups events under the app. Category is a free-form
    // sub-bucket; "default" is fine for everything. Wrapped in `Option`
    // because `#[cfg]` can't sit on an expression inside a `.with()` chain
    // — `Option<Layer>` is a no-op Layer when `None`, so the chain still
    // type-checks on non-macOS targets.
    let oslog_layer: Option<tracing_oslog::OsLogger> = if cfg!(target_os = "macos") {
        Some(tracing_oslog::OsLogger::new("com.s-voice.app", "default"))
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .with(oslog_layer)
        .init();

    let _ = FILTER_HANDLE.set(handle);

    // Make sure the very first line lands even if subsequent init code panics.
    eprintln!(
        "logging initialized (debug={}, file={})",
        debug_enabled,
        log_file.display()
    );
    tracing::info!(
        "logging initialized (debug={}, file={})",
        debug_enabled,
        log_file.display()
    );
    true
}

/// Flip the file/stderr filter between info and debug. Cheap, can be called
/// from any thread, takes effect on the next emitted event.
pub fn set_debug(debug_enabled: bool) -> Result<(), String> {
    let handle = FILTER_HANDLE
        .get()
        .ok_or_else(|| "logging not initialized".to_string())?;
    let filter = if debug_enabled {
        EnvFilter::new("s_voice_lib=debug,info")
    } else {
        EnvFilter::new("s_voice_lib=info,warn")
    };
    handle
        .reload(filter)
        .map_err(|e| format!("filter reload failed: {e}"))?;
    let state = if debug_enabled { "ON" } else { "OFF" };
    tracing::info!("debug mode toggled: {state}");
    Ok(())
}
