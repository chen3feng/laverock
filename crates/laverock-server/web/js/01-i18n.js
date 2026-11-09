// ---------------------------------------------------------------- i18n
//
// Runtime for the strings registered by `web/i18n/*.js`.

let lang = 'zh';
const t = (key, params) => {
  let s = (I18N[lang] && I18N[lang][key]) ?? I18N.zh[key] ?? key;
  if (params) for (const [k, v] of Object.entries(params)) s = s.replaceAll(`{${k}}`, v);
  return s;
};
const dateLocale = () => (t("ui.en_us"));

function detectLang(ignoreSaved = false) {
  const saved = ignoreSaved ? null : localStorage.getItem('laverock.lang');
  if (saved === 'zh' || saved === 'en') return saved;
  return (navigator.language || 'zh').toLowerCase().startsWith('zh') ? 'zh' : 'en';
}

// Static markup carries translations in data-i18n / -ph / -title attributes.
function applyI18n(root = document) {
  root.querySelectorAll('[data-i18n]').forEach((el) => { el.innerHTML = t(el.dataset.i18n); });
  root.querySelectorAll('[data-i18n-ph]').forEach((el) => { el.placeholder = t(el.dataset.i18nPh); });
  root.querySelectorAll('[data-i18n-title]').forEach((el) => { el.title = t(el.dataset.i18nTitle); });
  document.documentElement.lang = lang === 'zh' ? 'zh-CN' : 'en';
}

async function setLanguage(next, { persist = true } = {}) {
  if (next !== 'zh' && next !== 'en') return;
  lang = next;
  if (persist === 'explicit') localStorage.setItem('laverock.lang', lang);
  else if (persist === 'system') localStorage.removeItem('laverock.lang');
  $('langSelect').value = persist === 'explicit' ? lang : 'system';
  $('settingsLang').value = $('langSelect').value;
  applyI18n();
  document.title = `${t('nav.' + currentPage())} · Laverock`;
  // Rebuild every dynamic, localized view.
  fillPreset($('hfPreset'), HF_PRESETS);
  fillPreset($('pipPreset'), PIP_PRESETS);
  syncProxyMode();
  await Promise.all([loadEngines(), loadProfiles(), loadCaptures(), loadSettingsPage(), loadToolsPage(), loadProjects()]);
  renderHistory();
  if (currentPage() === 'transcribe') {
    syncTranscribePage();
    loadRecentTranscriptions();
  }
  if (currentPage() === 'synthesize') loadRecentResults('tts', 'synthRecent', 'synthRecentTitle');
  if (currentPage() === 'music') loadRecentResults('music', 'musicRecent', 'musicRecentTitle');
  if (currentPage() === 'convert') loadRecentResults('vc', 'convertRecent', 'convertRecentTitle');
  await loadModels();
  if (session && session.lan_enabled) loadDevices();
}

$('settingsLang').onchange = () => {
  $('langSelect').value = $('settingsLang').value;
  $('langSelect').dispatchEvent(new Event('change'));
};

$('langSelect').onchange = async () => {
  const v = $('langSelect').value;
  if (v === 'system') {
    await setLanguage(detectLang(true), { persist: 'system' });
  } else {
    await setLanguage(v, { persist: 'explicit' });
    // Best effort: also store it as the machine default so fresh paired
    // devices inherit the language. Phones cannot reach this endpoint.
    if (session.loopback && serverSettings) {
      try {
        await api('/api/settings', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ ...serverSettings, language: v }),
        });
      } catch (_) { /* per-device setting already saved locally */ }
    }
  }
};

// Manifest text is either a plain string (third-party manifests) or a
// {en, zh} object (everything we generate). Resolve it for the UI
// language; engine-provided ids stay untouched when no label exists.
function loc(value) {
  if (value == null) return '';
  if (typeof value === 'string') return value;
  return value[lang] || value.zh || value.en || '';
}


let engines = [];
let profiles = [];
let session = { loopback: true, paired: false };
let modelsData = null;
let modelsTimer = null;
let previewId = null;
let toastTimer = null;
let mp3Supported = false;
let transcribeAvailable = false;
let ffmpegAvailable = false;
let lastSynthBlob = null;
let serverSettings = null;
