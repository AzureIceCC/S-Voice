//! Audio capture via cpal, offloaded to a dedicated thread.
//!
//! `cpal::Stream` is `!Send`, so we can't share it across tokio tasks. Instead:
//! - the worker thread owns the Stream and the in-flight sample buffer
//! - the public `AudioController` is `Send + Sync` and exposes Start/Stop/level
//! - commands and responses travel over `std::sync::mpsc`

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, Stream, StreamConfig, SupportedStreamConfigRange};
use hound::{SampleFormat as HoundFormat, WavSpec, WavWriter};
use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thiserror::Error;

const LEVEL_WINDOW: usize = 480; // ~30ms at 16kHz
const AUDIO_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const TARGET_SAMPLE_RATE: u32 = 16000;

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
    #[error("audio worker did not respond to {0} within the timeout")]
    WorkerTimeout(&'static str),
    #[error("audio worker init failed: {0}")]
    WorkerInit(String),
}

#[derive(Clone, Debug)]
struct SelectedInputConfig {
    config: StreamConfig,
    sample_format: SampleFormat,
}

#[derive(Clone, Copy, Debug)]
struct InputConfigCandidate {
    sample_format: SampleFormat,
    channels: u16,
    min_sample_rate: u32,
    max_sample_rate: u32,
}

struct WorkerState {
    stream: Option<Stream>,
    samples: Vec<f32>,
    input_config: SelectedInputConfig,
    /// Original channel count from cpal. Kept for logging/diagnostics only;
    /// the actual `samples` Vec is already mono-downmixed by `on_input`, so
    /// downstream code must NOT re-downmix (a previous version did, which
    /// halved the captured duration on stereo input devices).
    #[allow(dead_code)]
    channels: u16,
    level_meter: LevelMeter,
    // Shared buffers for the audio callback (which is on a different thread).
    shared_samples: Option<std::sync::Arc<parking_lot::Mutex<Vec<f32>>>>,
    shared_level_meter: Option<std::sync::Arc<parking_lot::Mutex<LevelMeter>>>,
}

struct LevelMeter {
    values: [f32; LEVEL_WINDOW],
    cursor: usize,
    len: usize,
    sum_sq: f32,
}

impl LevelMeter {
    fn new() -> Self {
        Self {
            values: [0.0; LEVEL_WINDOW],
            cursor: 0,
            len: 0,
            sum_sq: 0.0,
        }
    }

    fn push(&mut self, sample: f32) {
        if self.len < LEVEL_WINDOW {
            self.len += 1;
        } else {
            let old = self.values[self.cursor];
            self.sum_sq -= old * old;
        }
        self.values[self.cursor] = sample;
        self.cursor = (self.cursor + 1) % LEVEL_WINDOW;
        self.sum_sq += sample * sample;
    }

    fn rms(&self) -> f32 {
        if self.len == 0 {
            0.0
        } else {
            (self.sum_sq.max(0.0) / self.len as f32).sqrt()
        }
    }

    fn clear(&mut self) {
        self.cursor = 0;
        self.len = 0;
        self.sum_sq = 0.0;
    }
}

enum Cmd {
    Start(SyncSender<Result<(), AudioError>>),
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
        let input_config = select_input_config(&supported).ok_or_else(|| {
            (
                AudioError::Init("no supported input config".into()),
                AudioInitKind::Recoverable,
            )
        })?;

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
                worker_main(cmd_rx, level_w, device, input_config);
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
        let (rsp_tx, rsp_rx) = std::sync::mpsc::sync_channel(1);
        self.cmd_tx
            .send(Cmd::Start(rsp_tx))
            .map_err(|_| AudioError::WorkerGone)?;
        tracing::info!("audio: start command sent to worker");
        receive_worker_response(rsp_rx, AUDIO_COMMAND_TIMEOUT, "Start")
    }

    pub fn stop(&self) -> Result<Vec<u8>, AudioError> {
        let (rsp_tx, rsp_rx) = std::sync::mpsc::sync_channel(1);
        self.cmd_tx
            .send(Cmd::Stop(rsp_tx))
            .map_err(|_| AudioError::WorkerGone)?;
        // Bound the wait so a stuck worker can't deadlock the pipeline.
        receive_worker_response(rsp_rx, AUDIO_COMMAND_TIMEOUT, "Stop")
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

fn receive_worker_response<T>(
    rsp_rx: Receiver<Result<T, AudioError>>,
    timeout: Duration,
    operation: &'static str,
) -> Result<T, AudioError> {
    match rsp_rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            tracing::error!(
                "audio worker did not respond to {operation} within {timeout:?}; forcing error"
            );
            Err(AudioError::WorkerTimeout(operation))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(AudioError::WorkerGone),
    }
}

fn select_input_config(ranges: &[SupportedStreamConfigRange]) -> Option<SelectedInputConfig> {
    let candidates: Vec<_> = ranges
        .iter()
        .map(|range| InputConfigCandidate {
            sample_format: range.sample_format(),
            channels: range.channels(),
            min_sample_rate: range.min_sample_rate().0,
            max_sample_rate: range.max_sample_rate().0,
        })
        .collect();
    let selected = select_candidate(&candidates)?;
    let sample_rate = TARGET_SAMPLE_RATE
        .max(selected.min_sample_rate)
        .min(selected.max_sample_rate);
    Some(SelectedInputConfig {
        config: StreamConfig {
            channels: selected.channels,
            sample_rate: SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        },
        sample_format: selected.sample_format,
    })
}

fn select_candidate(candidates: &[InputConfigCandidate]) -> Option<InputConfigCandidate> {
    candidates
        .iter()
        .find(|candidate| candidate.sample_format == SampleFormat::F32)
        .or_else(|| candidates.first())
        .copied()
}

fn worker_main(
    cmd_rx: Receiver<Cmd>,
    level: Arc<AtomicU32>,
    device: cpal::Device,
    input_config: SelectedInputConfig,
) {
    let mut state: Option<WorkerState> = match build_stream(input_config) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::error!("audio worker init failed: {e}");
            None
        }
    };

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            Cmd::Start(rsp) => {
                let result = match state.as_mut() {
                    Some(s) => start_capture(s, &device, &level),
                    None => Err(AudioError::WorkerInit("recorder not initialized".into())),
                };
                if let Err(e) = &result {
                    tracing::error!("start failed: {e}");
                }
                let _ = rsp.send(result);
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

fn build_stream(input_config: SelectedInputConfig) -> Result<WorkerState, AudioError> {
    // We just allocate state here; the actual stream is built on Start
    // so the worker can survive temporary stream errors.
    Ok(WorkerState {
        stream: None,
        samples: Vec::with_capacity(input_config.config.sample_rate.0 as usize * 30),
        channels: input_config.config.channels,
        input_config,
        level_meter: LevelMeter::new(),
        shared_samples: None,
        shared_level_meter: None,
    })
}

fn start_capture(
    state: &mut WorkerState,
    device: &cpal::Device,
    level: &Arc<AtomicU32>,
) -> Result<(), AudioError> {
    let config = state.input_config.config.clone();
    let sample_format = state.input_config.sample_format;
    let sample_rate = config.sample_rate.0;
    let channels = config.channels;

    let level_w = Arc::clone(level);
    let err_fn = |err| tracing::error!("cpal stream error: {err}");

    // We need a way to push samples into the Worker's Vec. Use Arc<Mutex<Vec>>.
    // But the WorkerState.samples is owned. Let's use a shared Vec.
    let samples = std::sync::Arc::new(parking_lot::Mutex::new(Vec::with_capacity(
        sample_rate as usize * 30,
    )));
    let samples_w = std::sync::Arc::clone(&samples);
    let level_meter = std::sync::Arc::new(parking_lot::Mutex::new(LevelMeter::new()));
    let level_meter_w = std::sync::Arc::clone(&level_meter);

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| {
                on_input(&samples_w, &level_meter_w, &level_w, data, channels, |s| s)
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => {
            let samples_w = std::sync::Arc::clone(&samples);
            let level_meter_w = std::sync::Arc::clone(&level_meter);
            device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    on_input(&samples_w, &level_meter_w, &level_w, data, channels, |s| {
                        s as f32 / i16::MAX as f32
                    });
                },
                err_fn,
                None,
            )
        }
        SampleFormat::U16 => {
            let samples_w = std::sync::Arc::clone(&samples);
            let level_meter_w = std::sync::Arc::clone(&level_meter);
            device.build_input_stream(
                &config,
                move |data: &[u16], _| {
                    on_input(&samples_w, &level_meter_w, &level_w, data, channels, |s| {
                        (s as f32 - 32768.0) / 32768.0
                    });
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
    state.level_meter.clear();
    state.stream = Some(stream);
    state.shared_samples = Some(samples);
    state.shared_level_meter = Some(level_meter);
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
    state.shared_level_meter = None;
    state.level_meter.clear();

    if samples.is_empty() {
        return Ok(Vec::new());
    }

    // `samples` is already mono-downmixed by `on_input` (the audio callback
    // averages per-frame into a flat `Vec<f32>`). Do NOT re-downmix here
    // — doing so would halve the duration on stereo input devices.
    // Convert the f32 samples to 16-bit PCM with clamping, then encode.
    let pcm = f32_to_pcm16(&samples);

    let spec = WavSpec {
        channels: 1,
        sample_rate: state.input_config.config.sample_rate.0,
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

fn on_input<T, F>(
    samples: &std::sync::Arc<parking_lot::Mutex<Vec<f32>>>,
    level_meter: &std::sync::Arc<parking_lot::Mutex<LevelMeter>>,
    level: &Arc<AtomicU32>,
    data: &[T],
    channels: u16,
    to_f32: F,
) where
    T: Copy,
    F: Fn(T) -> f32,
{
    let channels = usize::from(channels.max(1));
    let mut captured = samples.lock();
    let mut meter = level_meter.lock();
    for frame in data.chunks(channels) {
        let mono = frame.iter().copied().map(&to_f32).sum::<f32>() / frame.len() as f32;
        captured.push(mono);
        meter.push(mono);
    }
    let rms = meter.rms();
    drop(meter);
    drop(captured);
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

    #[test]
    fn level_meter_matches_rolling_rms() {
        let mut meter = LevelMeter::new();
        let samples: Vec<f32> = (0..(LEVEL_WINDOW * 3 + 17))
            .map(|i| ((i % 31) as f32 - 15.0) / 15.0)
            .collect();

        for (index, sample) in samples.iter().copied().enumerate() {
            meter.push(sample);
            let start = (index + 1).saturating_sub(LEVEL_WINDOW);
            let expected = (samples[start..=index].iter().map(|x| x * x).sum::<f32>()
                / (index + 1 - start) as f32)
                .sqrt();
            assert!((meter.rms() - expected).abs() < 0.000_01);
        }
    }

    #[test]
    fn input_callback_converts_and_downmixes_without_temporary_buffer() {
        let samples = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let meter = Arc::new(parking_lot::Mutex::new(LevelMeter::new()));
        let level = Arc::new(AtomicU32::new(0.0_f32.to_bits()));

        on_input(
            &samples,
            &meter,
            &level,
            &[i16::MAX, i16::MAX, 0, i16::MAX],
            2,
            |s| s as f32 / i16::MAX as f32,
        );

        assert_eq!(*samples.lock(), vec![1.0, 0.5]);
        let expected_rms = ((1.0_f32 + 0.25) / 2.0).sqrt();
        assert!((meter.lock().rms() - expected_rms).abs() < f32::EPSILON);
    }

    fn candidate(
        sample_format: SampleFormat,
        channels: u16,
        min_sample_rate: u32,
        max_sample_rate: u32,
    ) -> InputConfigCandidate {
        InputConfigCandidate {
            sample_format,
            channels,
            min_sample_rate,
            max_sample_rate,
        }
    }

    #[test]
    fn input_config_prefers_f32_without_mixing_in_default_format() {
        let i16_default = candidate(SampleFormat::I16, 1, 8000, 48000);
        let f32_supported = candidate(SampleFormat::F32, 2, 44100, 96000);

        let selected = select_candidate(&[i16_default, f32_supported]).unwrap();

        assert_eq!(selected.sample_format, SampleFormat::F32);
        assert_eq!(selected.channels, 2);
        assert_eq!(selected.min_sample_rate, 44100);
    }

    #[test]
    fn selected_rate_is_clamped_to_supported_range() {
        let below_16k = candidate(SampleFormat::F32, 1, 8000, 12000);
        let above_16k = candidate(SampleFormat::F32, 1, 44100, 48000);

        let low_rate = TARGET_SAMPLE_RATE
            .max(below_16k.min_sample_rate)
            .min(below_16k.max_sample_rate);
        let high_rate = TARGET_SAMPLE_RATE
            .max(above_16k.min_sample_rate)
            .min(above_16k.max_sample_rate);

        assert_eq!(low_rate, 12000);
        assert_eq!(high_rate, 44100);
    }

    #[test]
    fn worker_response_propagates_start_success_and_failure() {
        let (success_tx, success_rx) = std::sync::mpsc::sync_channel(1);
        success_tx.send(Ok(())).unwrap();
        assert!(receive_worker_response(success_rx, Duration::from_millis(10), "Start").is_ok());

        let (failure_tx, failure_rx) = std::sync::mpsc::sync_channel::<Result<(), AudioError>>(1);
        failure_tx
            .send(Err(AudioError::Play("test failure".into())))
            .unwrap();
        assert!(matches!(
            receive_worker_response(failure_rx, Duration::from_millis(10), "Start"),
            Err(AudioError::Play(message)) if message == "test failure"
        ));
    }

    #[test]
    fn worker_response_distinguishes_timeout_and_disconnect() {
        let (_timeout_tx, timeout_rx) = std::sync::mpsc::sync_channel::<Result<(), AudioError>>(1);
        assert!(matches!(
            receive_worker_response(timeout_rx, Duration::from_millis(1), "Start"),
            Err(AudioError::WorkerTimeout("Start"))
        ));

        let (disconnected_tx, disconnected_rx) =
            std::sync::mpsc::sync_channel::<Result<(), AudioError>>(1);
        drop(disconnected_tx);
        assert!(matches!(
            receive_worker_response(disconnected_rx, Duration::from_millis(10), "Start"),
            Err(AudioError::WorkerGone)
        ));
    }

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
