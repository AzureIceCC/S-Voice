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
let uiLanguage = 'zh';
const FLOAT_TEXT = { zh: { recording:'语音输入中…', processing:'处理中…', polishing:'润色中…', error:'错误', waiting:'等待中', startFail:'STT 启动失败', starting:'STT 启动中…' }, en: { recording:'Voice input…', processing:'Processing…', polishing:'Polishing…', error:'Error', waiting:'Waiting', startFail:'STT startup failed', starting:'Starting STT…' } };
const ft = () => FLOAT_TEXT[uiLanguage] || FLOAT_TEXT.zh;

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
    $('rec-state').textContent = ft().recording;
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
    $('rec-state').textContent = ft().processing;
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
    $('rec-state').textContent = ft().polishing;
    wave.classList.add('polishing');
    wave.style.width = '100%';
  } else if (state === 'error') {
    $('rec-state').textContent = ft().error;
    wave.classList.add('error');
    wave.style.width = '100%';
    clearTimers();
    $('rec-time').textContent = '0.0s';
  } else {
    // idle / waiting — panel returns to the quiet "等待中" state. We do
    // NOT keep the just-pasted text visible here; the recognised content
    // is in the clipboard / already pasted, the panel just sits quietly.
    $('rec-state').textContent = ft().waiting;
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

async function loadUiLanguage() {
  try {
    const settings = await window.t.invoke('cmd_get_settings');
    if (settings && (settings.ui_language === 'zh' || settings.ui_language === 'en')) uiLanguage = settings.ui_language;
  } catch (e) { /* keep Chinese default */ }
}

// Bridge startup status: starting / ready / failed.
function applyBridgeStatus(status) {
  if (status === 'failed') {
    $('rec-state').textContent = ft().startFail;
    $('waveform').className = 'waveform-bar error';
    $('waveform').style.width = '100%';
  } else if (status === 'starting') {
    $('rec-state').textContent = ft().starting;
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
      await window.t.event.listen('settings-changed', (event) => {
        if (event.payload && (event.payload.ui_language === 'zh' || event.payload.ui_language === 'en')) {
          uiLanguage = event.payload.ui_language;
          fetchAndApplyState();
        }
      });
    }
  } catch (e) { /* ignore */ }
  await loadUiLanguage();
  fetchAndApplyState();
})();
