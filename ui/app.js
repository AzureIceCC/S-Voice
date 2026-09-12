// Settings window logic — talks to Rust via window.t.invoke.

const $ = (id) => document.getElementById(id);

// Apply a Settings object to the form fields. Shared by the initial
// load and the "reload from disk" button so they stay in sync.
function applySettings(s) {
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

async function loadSettings() {
  try {
    const s = await window.t.invoke('cmd_get_settings');
    applySettings(s);
  } catch (e) {
    showStatus('加载设置失败: ' + e, 'error');
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
    showStatus('已从磁盘重新加载', 'success');
  } catch (e) {
    showStatus('重新加载失败: ' + e, 'error');
  }
}

function readSettings() {
  return {
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
    showStatus('已保存', 'success');
  } catch (e) {
    showStatus('保存失败: ' + e, 'error');
  }
}

async function testStt() {
  showStatus('正在连接 STT 服务…', 'info');
  try {
    const msg = await window.t.invoke('cmd_test_stt');
    showStatus(msg, 'success');
  } catch (e) {
    // Release builds strip the debug-only `cmd_test_stt` handler; the
    // tauri invoke layer reports it as a missing command. Detect that
    // and surface a friendly message instead of a raw error.
    const text = String(e);
    if (text.includes('not found') || text.includes('not allowed')) {
      showStatus('STT 测试仅在开发版本可用', 'info');
    } else {
      showStatus('STT 测试失败: ' + e, 'error');
    }
  }
}

async function testPolish() {
  const sample = '那个，我想确认一下，就是嗯，呃明天下午3点我们开会讨论一下PRD的事情';
  showStatus('正在调用 Ollama 润色…', 'info');
  try {
    const result = await window.t.invoke('cmd_test_polish', { text: sample });
    showStatus('润色结果: ' + result, 'success');
  } catch (e) {
    const text = String(e);
    if (text.includes('not found') || text.includes('not allowed')) {
      showStatus('润色测试仅在开发版本可用', 'info');
    } else {
      showStatus('润色失败: ' + e, 'error');
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
  $('record-hotkey').textContent = '按任意键…';
  lastKey = null;
}

function stopCapture(value) {
  recording = false;
  $('hotkey').classList.remove('recording-hotkey');
  $('record-hotkey').classList.remove('recording');
  $('record-hotkey').textContent = '录制';
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
$('clear-hotkey').addEventListener('click', () => stopCapture('Cmd+['));
$('save').addEventListener('click', saveSettings);
$('reload').addEventListener('click', reloadSettings);
$('test-stt').addEventListener('click', testStt);
$('test-polish').addEventListener('click', testPolish);
$('open-log-dir').addEventListener('click', async () => {
  try {
    await window.t.invoke('cmd_open_log_dir');
  } catch (e) {
    showStatus('打开日志目录失败: ' + e, 'error');
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
  badge.textContent = state;
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
    }
  } catch (e) { /* initial snapshot still provides a useful state */ }
  loadState();
})();

// Init
loadSettings();
