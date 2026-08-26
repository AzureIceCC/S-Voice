//! Audio capture via cpal, offloaded to a dedicated thread.
//!
//! `cpal::Stream` is `!Send`, so we can't share it across tokio tasks. Instead:
//! - the worker thread owns the Stream and the in-flight sample buffer
//! - the public `AudioController` is `Send + Sync` and exposes Start/Stop/level
//! - commands and responses travel over `std::sync::mpsc`

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use hound::{SampleFormat as HoundFormat, WavSpec, WavWriter};
use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thiserror::Error;

const LEVEL_WINDOW: usize = 480; // ~30ms at 16kHz

/// Severity of an `AudioController` construction failure. Drives whether the
/// pipeline should keep trying to rebuild the controller on subsequent
/// attempts or give up and surface a permanent error to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioInitKind {
    /// A transient or device-specific failure that may succeed on retry
    /// (e.g. stream build failed, init query failed).
    Recoverable,
    /// A failure that retrying cannot fix: no input device, or the OS
    /// refused to spawn the worker thread. Caller should mark the
    /// recorder permanently failed and stop attempting rebuilds.
    Terminal,
}

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("no input device available")]
    NoInputDevice,
    #[error("cpal init failed: {0}")]
    Init(String),
    #[error("stream build failed: {0}")]
    Stream(String),
    #[error("playback failed: {0}")]
    Play(String),
    #[error("WAV encode failed: {0}")]
    Wav(String),
    #[error("audio worker thread failed to start: {0}")]
    WorkerSpawn(String),
    #[error("audio worker disconnected")]
    WorkerGone,
    #[error("audio worker init failed: {0}")]
    WorkerInit(String),
}

struct WorkerState {
    stream: Option<Stream>,
    samples: Vec<f32>,
    sample_rate: u32,
    /// Original channel count from cpal. Kept for logging/diagnostics only;
    /// the actual `samples` Vec is already mono-downmixed by `on_f32`, so
    /// downstream code must NOT re-downmix (a previous version did, which
    /// halved the captured duration on stereo input devices).
    #[allow(dead_code)]
    channels: u16,
    level_window: Vec<f32>,
    // Shared buffers for the audio callback (which is on a different thread).
    shared_samples: Option<std::sync::Arc<parking_lot::Mutex<Vec<f32>>>>,
    shared_level_window: Option<std::sync::Arc<parking_lot::Mutex<Vec<f32>>>>,
}

enum Cmd {
    Start,
    Stop(SyncSender<Result<Vec<u8>, AudioError>>),
    Shutdown,
}

pub struct AudioController {
    cmd_tx: Sender<Cmd>,
    pub level: Arc<AtomicU32>, // f32 bits
    /// Join handle for the dedicated worker thread, kept so `Drop` can
    /// call `join()` after the worker has actually exited.
    worker_handle: Arc<parking_lot::Mutex<Option<JoinHandle<()>>>>,
    /// Receiver half of a one-shot channel whose sender is owned by the
    /// worker thread. The sender is dropped when `worker_main` returns,
    /// so `recv_timeout` returns `Disconnected` exactly when the worker
    /// has fully exited its loop and released the cpal `Stream` /
    /// device handle.
    worker_done_rx: Arc<parking_lot::Mutex<Option<Receiver<()>>>>,
}

impl AudioController {
    pub fn new() -> Result<Self, (AudioError, AudioInitKind)> {
        let host = cpal::default_host();
        let device = match host.default_input_device() {
            Some(d) => d,
            None => return Err((AudioError::NoInputDevice, AudioInitKind::Terminal)),
        };
        let supported: Vec<_> = match device.supported_input_configs() {
            Ok(it) => it.collect(),
            Err(e) => return Err((AudioError::Init(e.to_string()), AudioInitKind::Recoverable)),
        };
        let config = match supported
            .iter()
            .find(|c| c.sample_format() == SampleFormat::F32)
            .or(supported.first())
        {
            Some(c) => c,
            None => {
                return Err((
                    AudioError::Init("no supported input config".into()),
                    AudioInitKind::Recoverable,
                ))
            }
        };

        let sample_rate = config.min_sample_rate().0.max(16000);
        let channels = config.channels();

        let (cmd_tx, cmd_rx) = channel::<Cmd>();
        let level = Arc::new(AtomicU32::new(0));
        let worker_handle: Arc<parking_lot::Mutex<Option<JoinHandle<()>>>> =
            Arc::new(parking_lot::Mutex::new(None));
        let (done_tx, done_rx) = channel::<()>();
        let worker_done_rx: Arc<parking_lot::Mutex<Option<Receiver<()>>>> =
            Arc::new(parking_lot::Mutex::new(Some(done_rx)));

        let level_w = Arc::clone(&level);
        let handle_slot = Arc::clone(&worker_handle);
        let join = thread::Builder::new()
            .name("audio-worker".into())
            .spawn(move || {
                worker_main(cmd_rx, level_w, device, sample_rate, channels);
                // Drop the done sender here so the controller's
                // `recv_timeout` unblocks with `Disconnected`.
                drop(done_tx);
            })
            .map_err(|e| {
                (
                    AudioError::WorkerSpawn(e.to_string()),
                    AudioInitKind::Terminal,
                )
            })?;
        *handle_slot.lock() = Some(join);

        Ok(Self {
            cmd_tx,
            level,
            worker_handle,
            worker_done_rx,
        })
    }

    pub fn start(&self) -> Result<(), AudioError> {
        self.cmd_tx
            .send(Cmd::Start)
            .map_err(|_| AudioError::WorkerGone)?;
        tracing::info!("audio: start command sent to worker");
        Ok(())
    }

    pub fn stop(&self) -> Result<Vec<u8>, AudioError> {
        let (rsp_tx, rsp_rx) = std::sync::mpsc::sync_channel(1);
        self.cmd_tx
            .send(Cmd::Stop(rsp_tx))
            .map_err(|_| AudioError::WorkerGone)?;
        // Bound the wait so a stuck worker can't deadlock the pipeline.
        match rsp_rx.recv_timeout(std::time::Duration::from_secs(3)) {
            Ok(r) => r,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                tracing::error!("audio worker did not respond to Stop within 3s; forcing error");
                Err(AudioError::WorkerGone)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(AudioError::WorkerGone),
        }
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

impl Drop for AudioController {
    fn drop(&mut self) {
        // Ask the worker to exit, then wait briefly for it to honor the
        // request. The done-channel unblocks with `Disconnected` once the
        // worker has dropped its `done_tx` sender on its way out, which
        // only happens after the cpal `Stream` and `Device` have been
        // released by `WorkerState`'s `Drop`. If the worker is wedged
        // inside `stop_capture` it will not see the `Cmd::Shutdown` and
        // the done sender stays alive, so we cap the wait at 500ms and
        // then *detach* the worker thread instead of joining it: a
        // wedged worker that ignores Shutdown would otherwise block
        // `Drop` forever on a `join()` that has no timeout.
        let _ = self.cmd_tx.send(Cmd::Shutdown);
        let mut clean_exit = false;
        if let Some(rx) = self.worker_done_rx.lock().take() {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(()) => {
                    clean_exit = true;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    // Worker has exited and dropped its sender, but did
                    // not send anything. Treat as clean exit.
                    clean_exit = true;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    tracing::error!(
                        "audio worker did not exit within 500ms of Shutdown; \
                         detaching thread (OS will reclaim on process exit)"
                    );
                }
            }
        }
        // Consume the join handle so its `JoinGuard` does not dangle.
        // - Clean exit: the worker has already returned, so `join()`
        //   returns immediately and the thread's resources are released
        //   deterministically.
        // - Timed-out / wedged worker: `join()` would block forever, so
        //   we skip it. `JoinHandle`'s `Drop` (since Rust 1.49) detaches
        //   the thread instead, letting the OS reclaim it on process
        //   exit. The thread keeps running but `Drop` returns and the
        //   main flow is unblocked.
        if let Some(handle) = self.worker_handle.lock().take() {
            if clean_exit {
                let _ = handle.join();
            } else {
                tracing::error!(
                    "audio worker still running at Drop; letting JoinHandle drop \
                     (auto-detaches the thread, OS reclaims on process exit)"
                );
                // Intentionally do not `join`; the `JoinHandle` is dropped
                // here, which detaches the underlying thread.
                drop(handle);
            }
        }
    }
}

fn worker_main(
    cmd_rx: Receiver<Cmd>,
    level: Arc<AtomicU32>,
    device: cpal::Device,
    sample_rate: u32,
    channels: u16,
) {
    let mut state: Option<WorkerState> = match build_stream(&device, &level, sample_rate, channels)
    {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::error!("audio worker init failed: {e}");
            None
        }
    };

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            Cmd::Start => {
                if let Some(s) = state.as_mut() {
                    if let Err(e) = start_capture(s, &device, sample_rate, channels, &level) {
                        tracing::error!("start failed: {e}");
                    }
                }
            }
            Cmd::Stop(rsp) => {
                let result = match state.as_mut() {
                    Some(s) => stop_capture(s),
                    None => Err(AudioError::WorkerInit("recorder not initialized".into())),
                };
                let _ = rsp.send(result);
            }
            Cmd::Shutdown => break,
        }
    }
}

fn build_stream(
    _device: &cpal::Device,
    _level: &Arc<AtomicU32>,
    _sample_rate: u32,
    _channels: u16,
) -> Result<WorkerState, AudioError> {
    // We just allocate state here; the actual stream is built on Start
    // so the worker can survive temporary stream errors.
    Ok(WorkerState {
        stream: None,
        samples: Vec::with_capacity(_sample_rate as usize * 30),
        sample_rate: _sample_rate,
        channels: _channels,
        level_window: Vec::with_capacity(LEVEL_WINDOW),
        shared_samples: None,
        shared_level_window: None,
    })
}

fn start_capture(
    state: &mut WorkerState,
    device: &cpal::Device,
    sample_rate: u32,
    channels: u16,
    level: &Arc<AtomicU32>,
) -> Result<(), AudioError> {
    let config = StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let level_w = Arc::clone(level);
    let err_fn = |err| tracing::error!("cpal stream error: {err}");

    // We need a way to push samples into the Worker's Vec. Use Arc<Mutex<Vec>>.
    // But the WorkerState.samples is owned. Let's use a shared Vec.
    let samples = std::sync::Arc::new(parking_lot::Mutex::new(Vec::with_capacity(
        sample_rate as usize * 30,
    )));
    let samples_w = std::sync::Arc::clone(&samples);
    let level_window =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::with_capacity(LEVEL_WINDOW)));
    let level_window_w = std::sync::Arc::clone(&level_window);

    let stream = match device
        .default_input_config()
        .map_err(|e| AudioError::Init(e.to_string()))?
        .sample_format()
    {
        SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| on_f32(&samples_w, &level_window_w, &level_w, data, channels),
            err_fn,
            None,
        ),
        SampleFormat::I16 => {
            let samples_w = std::sync::Arc::clone(&samples);
            let level_window_w = std::sync::Arc::clone(&level_window);
            device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    let f: Vec<f32> = data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                    on_f32(&samples_w, &level_window_w, &level_w, &f, channels);
                },
                err_fn,
                None,
            )
        }
        SampleFormat::U16 => {
            let samples_w = std::sync::Arc::clone(&samples);
            let level_window_w = std::sync::Arc::clone(&level_window);
            device.build_input_stream(
                &config,
                move |data: &[u16], _| {
                    let f: Vec<f32> = data
                        .iter()
                        .map(|s| (*s as f32 - 32768.0) / 32768.0)
                        .collect();
                    on_f32(&samples_w, &level_window_w, &level_w, &f, channels);
                },
                err_fn,
                None,
            )
        }
        f => return Err(AudioError::Init(format!("unsupported format: {f:?}"))),
    }
    .map_err(|e| AudioError::Stream(e.to_string()))?;

    stream.play().map_err(|e| AudioError::Play(e.to_string()))?;

    // Stash the shared buffers into the WorkerState so Stop can drain them.
    state.samples = Vec::new();
    state.level_window.clear();
    state.stream = Some(stream);
    state.shared_samples = Some(samples);
    state.shared_level_window = Some(level_window);
    Ok(())
}

fn stop_capture(state: &mut WorkerState) -> Result<Vec<u8>, AudioError> {
    // Drop the stream to stop capture.
    state.stream = None;
    let samples = if let Some(shared) = state.shared_samples.take() {
        std::mem::take(&mut *shared.lock())
    } else {
        std::mem::take(&mut state.samples)
    };
    state.shared_level_window = None;
    state.level_window.clear();

    if samples.is_empty() {
        return Ok(Vec::new());
    }

    // `samples` is already mono-downmixed by `on_f32` (the audio callback
    // averages per-frame into a flat `Vec<f32>`). Do NOT re-downmix here
    // — doing so would halve the duration on stereo input devices.
    // Convert the f32 samples to 16-bit PCM with clamping, then encode.
    let pcm = f32_to_pcm16(&samples);

    let spec = WavSpec {
        channels: 1,
        sample_rate: state.sample_rate,
        bits_per_sample: 16,
        sample_format: HoundFormat::Int,
    };
    let mut buf = Cursor::new(Vec::with_capacity(pcm.len() * 2 + 64));
    {
        let mut w = WavWriter::new(&mut buf, spec).map_err(|e| AudioError::Wav(e.to_string()))?;
        for s in &pcm {
            w.write_sample(*s)
                .map_err(|e| AudioError::Wav(e.to_string()))?;
        }
        w.finalize().map_err(|e| AudioError::Wav(e.to_string()))?;
    }
    Ok(buf.into_inner())
}

fn on_f32(
    samples: &std::sync::Arc<parking_lot::Mutex<Vec<f32>>>,
    level_window: &std::sync::Arc<parking_lot::Mutex<Vec<f32>>>,
    level: &Arc<AtomicU32>,
    data: &[f32],
    channels: u16,
) {
    // Downmix if multi-channel
    let mono: Vec<f32> = if channels <= 1 {
        data.to_vec()
    } else {
        let step = channels as usize;
        data.chunks(step)
            .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
            .collect()
    };
    {
        let mut s = samples.lock();
        s.extend_from_slice(&mono);
    }
    let rms = {
        let mut lw = level_window.lock();
        for s in &mono {
            if lw.len() >= LEVEL_WINDOW {
                lw.remove(0);
            }
            lw.push(*s);
        }
        if lw.is_empty() {
            0.0
        } else {
            let sum_sq: f32 = lw.iter().map(|x| x * x).sum();
            (sum_sq / lw.len() as f32).sqrt()
        }
    };
    // Light smoothing
    let prev = f32::from_bits(level.load(Ordering::Relaxed));
    let smoothed = prev * 0.5 + rms * 0.5;
    level.store(smoothed.to_bits(), Ordering::Relaxed);
}

/// Convert a slice of float samples in `[-1.0, 1.0]` to 16-bit signed PCM,
/// clamping any out-of-range values. Returns an empty vec for empty input.
fn f32_to_pcm16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Boundary values must round-trip through the i16 range without
    /// wrapping (i16::MAX + 1 wraps to i16::MIN if we forget to clamp).
    #[test]
    fn f32_to_pcm16_boundaries() {
        assert_eq!(f32_to_pcm16(&[0.0])[0], 0);
        assert_eq!(f32_to_pcm16(&[1.0])[0], i16::MAX);
        assert_eq!(f32_to_pcm16(&[-1.0])[0], -i16::MAX);
    }

    /// Anything outside `[-1.0, 1.0]` must clamp, NOT wrap.
    /// A previous version cast `s * 32767.0` directly to i16, which
    /// undefined-behaviour-wrapped values > 1.0 to negative numbers.
    /// If someone refactors this helper, this test catches the regression.
    #[test]
    fn f32_to_pcm16_clamps_out_of_range() {
        assert_eq!(f32_to_pcm16(&[2.0])[0], i16::MAX, "above 1.0 must clamp up");
        assert_eq!(
            f32_to_pcm16(&[-2.0])[0],
            -i16::MAX,
            "below -1.0 must clamp down"
        );
        assert_eq!(f32_to_pcm16(&[f32::INFINITY])[0], i16::MAX);
        assert_eq!(f32_to_pcm16(&[f32::NEG_INFINITY])[0], -i16::MAX);
    }

    /// Empty input returns an empty vec (not panics, not a single zero).
    #[test]
    fn f32_to_pcm16_empty() {
        assert!(f32_to_pcm16(&[]).is_empty());
    }

    /// Output length always matches input length. Important because
    /// `hound` writes one i16 per sample and a length mismatch would
    /// desync the WAV header's `data` chunk.
    #[test]
    fn f32_to_pcm16_preserves_length() {
        for n in [0, 1, 100, 16000, 48000] {
            let input = vec![0.0f32; n];
            assert_eq!(f32_to_pcm16(&input).len(), n, "n={n}");
        }
    }
}
