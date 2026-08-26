// Minimal Tauri 2 invoke shim — Tauri exposes window.__TAURI__ globally.
// This file just guards against its absence for non-Tauri previews.
(function () {
  if (!window.__TAURI__) {
    console.warn('Tauri runtime not available — running in browser preview?');
  }
  window.t = window.__TAURI__ || {};
  window.t.invoke = (window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke)
    || (async (cmd, args) => { throw new Error('Tauri not available: ' + cmd); });
  window.t.listen = (window.__TAURI__ && window.__TAURI__.event && window.__TAURI__.event.listen)
    || (async () => () => {});
  window.t.event = window.__TAURI__ && window.__TAURI__.event;
})();
