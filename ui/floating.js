// Floating recording panel — driven by Tauri events from the pipeline.

const $ = (id) => document.getElementById(id);

// Two independent clocks: one for the user's recording duration, one for
// the pipeline's processing duration (STT + polish + paste). They never
// overlap (the pipeline only goes Recording → Processing → Idle), so a
// single DOM element is fine — the displayed value just swaps which clock
// it reads from. Idle keeps the last processing time for ~3s so the user
// can see how long the turn took, then resets.
let recordingStart = null;   // ms timestamp when current Recording began, else null
let processingStart = null;  // ms timestamp when current Processing began, else null
let lastProcessingSec = 0;   // most recent processing duration, shown briefly in idle
let timerInterval = null;

function updateTimer() {
  if (recordingStart !== null) {
    $('rec-time').textContent = ((Date.now() - recordingStart) / 1000).toFixed(1) + 's';
  } else if (processingStart !== null) {
    $('rec-time').textContent = ((Date.now() - processingStart) / 1000).toFixed(1) + 's';
  }
}

function clearTimers() {
  if (timerInterval !== null) {
    clearInterval(timerInterval);
    timerInterval = null;
  }
  recordingStart = null;
  processingStart = null;
}

function applyState(state) {
  const wave = $('waveform');
  wave.className = 'waveform-bar';
  if (state === 'recording') {
    $('rec-state').textContent = '语音输入中…';
    wave.classList.add('recording');
    if (timerInterval === null) {
      // Fresh recording: clear any stale processing time, start the
      // recording clock.
      processingStart = null;
      lastProcessingSec = 0;
      recordingStart = Date.now();
      timerInterval = setInterval(updateTimer, 100);
    }
  } else if (state === 'processing') {
    $('rec-state').textContent = '处理中…';
    wave.classList.add('processing');
    wave.style.width = '100%';
    if (recordingStart !== null) {
      // Recording just ended — freeze the recording time, start the
      // processing clock. The display now reflects the processing
      // duration, not the (now-paused) recording duration.
      recordingStart = null;
      processingStart = Date.now();
    }
  } else if (state === 'polishing') {
    // STT is done; Ollama is rewriting the transcript. Keep the
    // processing clock running so the user sees the polish leg too.
    $('rec-state').textContent = '润色中…';
    wave.classList.add('polishing');
    wave.style.width = '100%';
  } else if (state === 'error') {
    $('rec-state').textContent = '错误';
    wave.classList.add('error');
    wave.style.width = '100%';
    clearTimers();
    $('rec-time').textContent = '0.0s';
  } else {
    // idle / waiting — panel returns to the quiet "等待中" state. We do
    // NOT keep the just-pasted text visible here; the recognised content
    // is in the clipboard / already pasted, the panel just sits quietly.
    $('rec-state').textContent = '等待中';
    wave.style.width = '0%';
    if (processingStart !== null) {
      // Freeze the final processing duration so the user can see how
      // long the turn took before the panel resets.
      lastProcessingSec = (Date.now() - processingStart) / 1000;
      $('rec-time').textContent = lastProcessingSec.toFixed(1) + 's';
      clearTimers();
      // Fade the number out after a short beat so a fresh turn starts
      // from a clean "0.0s" baseline.
      setTimeout(() => {
        // Only clear if no new turn has started in the meantime.
        if (recordingStart === null && processingStart === null) {
          $('rec-time').textContent = '0.0s';
        }
      }, 3000);
    } else {
      clearTimers();
      $('rec-time').textContent = '0.0s';
    }
  }
}

async function fetchAndApplyState() {
  try {
    const state = await window.t.invoke('cmd_get_state');
    applyState(state);
  } catch (e) { /* ignore */ }
}

// Bridge startup status: starting / ready / failed.
function applyBridgeStatus(status) {
  if (status === 'failed') {
    $('rec-state').textContent = 'STT 启动失败';
    $('waveform').className = 'waveform-bar error';
    $('waveform').style.width = '100%';
  } else if (status === 'starting') {
    $('rec-state').textContent = 'STT 启动中…';
    $('waveform').className = 'waveform-bar processing';
    $('waveform').style.width = '100%';
  } else if (status === 'ready') {
    fetchAndApplyState();
  }
}

// Subscribe to pipeline events.
(async () => {
  try {
    if (window.t.event && window.t.event.listen) {
      await window.t.event.listen('state-changed', () => fetchAndApplyState());
      await window.t.event.listen('audio-level-changed', (event) => {
        const level = event.payload;
        if (typeof level !== 'number') return;
        const pct = Math.min(100, Math.max(0, level * 400));
        $('waveform').style.width = pct.toFixed(0) + '%';
      });
      await window.t.event.listen('bridge-status', (event) => {
        if (typeof event.payload === 'string') applyBridgeStatus(event.payload);
      });
    }
  } catch (e) { /* ignore */ }
  fetchAndApplyState();
})();
