// Settings window logic — talks to Rust via window.t.invoke.

const $ = (id) => document.getElementById(id);

// Apply a Settings object to the form fields. Shared by the initial
// load and the "reload from disk" button so they stay in sync.
function applySettings(s) {
  applyUiLanguage(s.ui_language || 'zh');
  $('ui-language').value = s.ui_language || 'zh';
  $('hotkey').value = s.hotkey || '';
  $('language').value = s.language || 'zh';
  $('stt-backend').value = s.stt_backend || 'apple_speech';
  $('stt-model').value = s.stt_model || '';
  $('ollama-model').value = s.ollama_model || '';
  $('ollama-keepalive').value = s.ollama_keep_alive || '30m';
  $('polish-enabled').checked = !!s.polish_enabled;
  $('polish-prompt').value = s.polish_prompt || '';
  // Streaming is always off for now (the on-radio is disabled). We still
  // restore the saved value so it round-trips correctly when v0.2 lands.
  $('streaming-' + (s.streaming_enabled ? 'on' : 'off')).checked = true;
  $('debug').checked = !!s.debug;
}

const UI_TEXT = {
  zh: {
    title: 'S-Voice — 设置', hotkeyLabel: '全局热键', record: '录制',
    clearHint: '支持单键或组合键（Cmd+[）', uiLabel: '显示语言',
    sttLang: 'STT 语言', auto: '自动检测', backend: '语音识别后端',
    apple: 'Apple SpeechAnalyzer（系统本地）', whisper: '本地 Whisper（离线）',
    backendHint: '后端选择是显式的；所选后端失败时不会自动切换。',
    model: '本地 Whisper 模型', ollamaModel: 'Ollama 模型', mode: '转写模式',
    once: '一次性转写（稳定，推荐）', realtime: '实时转写（开发中）',
    realtimeTitle: '实时转写暂未启用', modeHint: '实时转写暂未启用',
    polish: 'Ollama 润色', polishOn: '启用 LLM 润色',
    keep: 'Keep-alive (空闲多久后卸载)', prompt: '自定义润色 prompt（留空使用默认）',
    diagnostic: '诊断', debug: 'Debug 模式（详细日志）', log: '日志文件路径',
    finder: '在 Finder 中显示', save: '保存', reload: '从磁盘重新加载',
    testStt: '测试 STT 连接', testPolish: '测试润色', saved: '已保存',
    loadFail: '加载设置失败: ', reloadOk: '已从磁盘重新加载',
    reloadFail: '重新加载失败: ', saveFail: '保存失败: ', recordNow: '按任意键…',
    waiting: '等待中', recording: '语音输入中…', processing: '处理中…',
    polishing: '润色中…', error: '错误', sttConnecting: '正在连接 STT 服务…',
    sttDev: 'STT 测试仅在开发版本可用', sttFail: 'STT 测试失败: ',
    polishConnecting: '正在调用 Ollama 润色…', polishResult: '润色结果: ',
    polishDev: '润色测试仅在开发版本可用', polishFail: '润色失败: ',
    logFail: '打开日志目录失败: ', hotkeyPlaceholder: '点击右侧按钮录制', keepPlaceholder: '30m'
  },
  en: {
    title: 'S-Voice — Settings', hotkeyLabel: 'Global hotkey', record: 'Record',
    clearHint: 'Single key or combination (Cmd+[)', uiLabel: 'Display language',
    sttLang: 'STT language', auto: 'Auto detect', backend: 'Speech recognition backend',
    apple: 'Apple SpeechAnalyzer (system local)', whisper: 'Local Whisper (offline)',
    backendHint: 'Backend selection is explicit; failures never switch automatically.',
    model: 'Local Whisper model', ollamaModel: 'Ollama model', mode: 'Transcription mode',
    once: 'One-shot transcription (stable, recommended)', realtime: 'Realtime transcription (in development)',
    realtimeTitle: 'Realtime transcription is not enabled yet', modeHint: 'Realtime transcription is not enabled yet',
    polish: 'Ollama polish', polishOn: 'Enable LLM polish', keep: 'Keep-alive (unload after idle)',
    prompt: 'Custom polish prompt (blank uses default)', diagnostic: 'Diagnostics',
    debug: 'Debug mode (verbose logs)', log: 'Log file path', finder: 'Show in Finder',
    save: 'Save', reload: 'Reload from disk', testStt: 'Test STT connection',
    testPolish: 'Test polish', saved: 'Saved', loadFail: 'Failed to load settings: ',
    reloadOk: 'Reloaded from disk', reloadFail: 'Reload failed: ', saveFail: 'Save failed: ',
    recordNow: 'Press any key…', waiting: 'Waiting', recording: 'Voice input…',
    processing: 'Processing…', polishing: 'Polishing…', error: 'Error',
    sttConnecting: 'Connecting to STT service…', sttDev: 'STT test is available in development builds only',
    sttFail: 'STT test failed: ', polishConnecting: 'Calling Ollama polish…', polishResult: 'Polish result: ',
    polishDev: 'Polish test is available in development builds only', polishFail: 'Polish failed: ',
    logFail: 'Failed to open log directory: ', hotkeyPlaceholder: 'Click Record to capture', keepPlaceholder: '30m'
  }
};
function applyUiLanguage(lang) {
  const t = UI_TEXT[lang] || UI_TEXT.zh;
  document.documentElement.lang = lang === 'en' ? 'en' : 'zh-CN';
  document.title = t.title;
  const map = { '#hotkey-label':'hotkeyLabel','#record-hotkey':'record','#hotkey-hint':'clearHint','#ui-language-label':'uiLabel','#stt-language-label':'sttLang','#auto-language':'auto','#backend-label':'backend','#apple-option':'apple','#whisper-option':'whisper','#backend-hint':'backendHint','#stt-model-label':'model','#ollama-model-label':'ollamaModel','#mode-label':'mode','#once-label':'once','#realtime-label':'realtime','#streaming-hint':'modeHint','#polish-title':'polish','#polish-label':'polishOn','#keepalive-label':'keep','#prompt-summary':'prompt','#diagnostic-title':'diagnostic','#debug-label':'debug','#log-label':'log','#open-log-dir':'finder','#save':'save','#reload':'reload','#test-stt':'testStt','#test-polish':'testPolish' };
  Object.entries(map).forEach(([sel,key]) => { const el=document.querySelector(sel); if(el) el.textContent=t[key]; });
  $('hotkey').placeholder = t.hotkeyPlaceholder;
  $('ollama-keepalive').placeholder = t.keepPlaceholder;
  $('realtime-label-wrap').title = t.realtimeTitle;
  if (recording) $('record-hotkey').textContent = t.recordNow;
  window.currentUiText = t;
  const badge = $('state-badge');
  if (badge.dataset.state) applyStateBadge(badge.dataset.state);
}

async function loadSettings() {
  try {
    const s = await window.t.invoke('cmd_get_settings');
    applySettings(s);
  } catch (e) {
    showStatus((window.currentUiText||UI_TEXT.zh).loadFail + e, 'error');
  }
  // Log path is independent of the settings struct.
  try {
    $('log-path').value = await window.t.invoke('cmd_log_path');
  } catch (e) { /* ignore */ }
}

/// Re-read `settings.json` from disk and refresh the form. Pairs with
/// the diff-skip in `Settings::save` (#11) — together they let a user
/// (or automation) edit settings.json directly without losing the
/// change to a subsequent save or quit.
async function reloadSettings() {
  try {
    const s = await window.t.invoke('cmd_reload_settings');
    applySettings(s);
    showStatus((window.currentUiText||UI_TEXT.zh).reloadOk, 'success');
  } catch (e) {
    showStatus((window.currentUiText||UI_TEXT.zh).reloadFail + e, 'error');
  }
}

function readSettings() {
  return {
    ui_language: $('ui-language').value,
    hotkey: $('hotkey').value.trim() || 'Cmd+[',
    language: $('language').value,
    stt_backend: $('stt-backend').value,
    stt_model: $('stt-model').value.trim(),
    ollama_model: $('ollama-model').value.trim(),
    ollama_keep_alive: $('ollama-keepalive').value.trim() || '30m',
    polish_enabled: $('polish-enabled').checked,
    polish_prompt: $('polish-prompt').value,
    streaming_enabled: $('streaming-on').checked,    // always false in v0.1
    debug: $('debug').checked,
  };
}

async function saveSettings() {
  try {
    await window.t.invoke('cmd_update_settings', { newSettings: readSettings() });
    showStatus((window.currentUiText||UI_TEXT.zh).saved, 'success');
  } catch (e) {
    showStatus((window.currentUiText||UI_TEXT.zh).saveFail + e, 'error');
  }
}

async function testStt() {
  const t = window.currentUiText || UI_TEXT.zh;
  showStatus(t.sttConnecting, 'info');
  try {
    const msg = await window.t.invoke('cmd_test_stt');
    showStatus(msg, 'success');
  } catch (e) {
    // Release builds strip the debug-only `cmd_test_stt` handler; the
    // tauri invoke layer reports it as a missing command. Detect that
    // and surface a friendly message instead of a raw error.
    const text = String(e);
    if (text.includes('not found') || text.includes('not allowed')) {
      showStatus(t.sttDev, 'info');
    } else {
      showStatus(t.sttFail + e, 'error');
    }
  }
}

async function testPolish() {
  const sample = '那个，我想确认一下，就是嗯，呃明天下午3点我们开会讨论一下PRD的事情';
  const t = window.currentUiText || UI_TEXT.zh;
  showStatus(t.polishConnecting, 'info');
  try {
    const result = await window.t.invoke('cmd_test_polish', { text: sample });
    showStatus(t.polishResult + result, 'success');
  } catch (e) {
    const text = String(e);
    if (text.includes('not found') || text.includes('not allowed')) {
      showStatus(t.polishDev, 'info');
    } else {
      showStatus(t.polishFail + e, 'error');
    }
  }
}

// Hotkey capture: click "录制", then press the desired combo.
let recording = false;
let lastKey = null;
const CAPTURE_MODS = { 'Meta': 'Cmd', 'Alt': 'Alt', 'Shift': 'Shift', 'Control': 'Ctrl' };

function startCapture() {
  recording = true;
  $('hotkey').value = '';
  $('hotkey').classList.add('recording-hotkey');
  $('record-hotkey').classList.add('recording');
  $('record-hotkey').textContent = (window.currentUiText||UI_TEXT.zh).recordNow;
  lastKey = null;
}

function stopCapture(value) {
  recording = false;
  $('hotkey').classList.remove('recording-hotkey');
  $('record-hotkey').classList.remove('recording');
  $('record-hotkey').textContent = (window.currentUiText||UI_TEXT.zh).record;
  if (value) $('hotkey').value = value;
  $('clear-hotkey').hidden = !value;
}

function comboFromEvent(e) {
  const parts = [];
  if (e.metaKey) parts.push('Cmd');
  if (e.ctrlKey) parts.push('Ctrl');
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  const k = keyLabel(e);
  if (k) parts.push(k);
  return parts.join('+');
}

function keyLabel(e) {
  if (e.key.startsWith('Meta') || e.key.startsWith('Alt') ||
      e.key.startsWith('Shift') || e.key.startsWith('Control')) return null;
  if (e.key === ' ') return 'Space';
  if (e.key === 'Enter') return 'Enter';
  if (e.key === 'Escape') return 'Escape';
  if (e.key === 'Tab') return 'Tab';
  if (e.key === 'Backspace') return 'Backspace';
  if (e.key === 'Delete') return 'Delete';
  if (e.key === 'ArrowUp') return 'Up';
  if (e.key === 'ArrowDown') return 'Down';
  if (e.key === 'ArrowLeft') return 'Left';
  if (e.key === 'ArrowRight') return 'Right';
  if (/^F\d{1,2}$/.test(e.key)) return e.key;
  if (e.key.length === 1) {
    // Letters/digits/punctuation
    if (/^[a-zA-Z]$/.test(e.key)) return e.key.toUpperCase();
    return e.key;
  }
  return null;
}

document.addEventListener('keydown', (e) => {
  if (!recording) return;
  e.preventDefault();
  e.stopPropagation();
  const combo = comboFromEvent(e);
  if (combo) stopCapture(combo);
});

$('record-hotkey').addEventListener('click', startCapture);
// UI language is auto-saved on change: it's a pure-UI preference
// (no hardware/OS side effects) and the floating panel needs the
// update immediately to re-render its state labels. Other fields
// still require the explicit "保存" click because they trigger
// hotkey re-registration, model switches, etc. that the user
// might want to batch up before committing. `saveSettings` reads
// every field's current value and persists them all, so a stray
// hotkey still in some intermediate state could get flushed here
// too — acceptable because the in-memory form is what the user
// sees and intends to save eventually.
$('ui-language').addEventListener('change', () => saveSettings());
$('clear-hotkey').addEventListener('click', () => stopCapture('Cmd+['));
$('save').addEventListener('click', saveSettings);
$('reload').addEventListener('click', reloadSettings);
$('test-stt').addEventListener('click', testStt);
$('test-polish').addEventListener('click', testPolish);
$('open-log-dir').addEventListener('click', async () => {
  try {
    await window.t.invoke('cmd_open_log_dir');
  } catch (e) {
    showStatus((window.currentUiText||UI_TEXT.zh).logFail + e, 'error');
  }
});

function showStatus(msg, type) {
  const el = $('status');
  el.textContent = msg;
  el.className = 'status ' + (type || 'info');
  el.hidden = false;
  if (type === 'success') {
    setTimeout(() => { el.hidden = true; }, 4000);
  }
}

// State badge — initial snapshot plus live pipeline events.
function applyStateBadge(state) {
  const badge = $('state-badge');
  const t = window.currentUiText || UI_TEXT.zh;
  const labels = {
    idle: t.waiting,
    recording: t.recording,
    processing: t.processing,
    polishing: t.polishing,
    error: t.error,
  };
  badge.dataset.state = state;
  badge.textContent = labels[state] || state;
  badge.className = 'badge ' + state;
}

async function loadState() {
  try {
    const state = await window.t.invoke('cmd_get_state');
    applyStateBadge(state);
  } catch (e) { /* ignore */ }
}

(async () => {
  try {
    if (window.t.event && window.t.event.listen) {
      await window.t.event.listen('state-changed', (event) => {
        if (typeof event.payload === 'string') applyStateBadge(event.payload);
      });
      await window.t.event.listen('settings-changed', (event) => {
        if (event.payload && event.payload.ui_language) {
          applyUiLanguage(event.payload.ui_language);
          $('ui-language').value = event.payload.ui_language;
        }
      });
    }
  } catch (e) { /* initial snapshot still provides a useful state */ }
  loadState();
})();

// Init
loadSettings();
