// ---------------------------------------------------------------- routing / boot

function currentPage() {
  const raw = location.hash.replace(/^#\/?/, '');
  return PAGES.includes(raw) ? raw : 'synthesize';
}

function showPage(page) {
  for (const p of PAGES) $('page-' + p).classList.toggle('active', p === page);
  document.querySelectorAll('nav a').forEach((a) => a.classList.toggle('active', a.dataset.page === page));
  document.title = `${t('nav.' + page)} · Laverock`;
  if (page !== 'models' && modelsTimer) {
    clearInterval(modelsTimer);
    modelsTimer = null;
  }
  if (page === 'captures') loadCaptures();
  if (page === 'voices') loadProfiles();
  if (page === 'history') loadHistory();
  if (page === 'models') loadModels();
  if (page === 'settings') loadSettingsPage();
}

window.addEventListener('hashchange', () => showPage(currentPage()));

async function loadHealth() {
  try {
    const h = await api('/api/health');
    const v = (h.version || '').trim();
    $('version').textContent = v ? 'v' + v : '';
    $('aboutVersion').textContent = v ? 'v' + v : '—';
    $('connState').classList.remove('off');
    mp3Supported = !!h.mp3;
    transcribeAvailable = !!h.transcribe;
    for (const cap of [synthCapture, voiceCapture, libraryCapture]) cap.syncTranscribeButton();
  } catch (_) {
    $('connState').classList.add('off');
  }
}

async function boot() {
  lang = detectLang();
  applyI18n();
  fillPreset($('hfPreset'), HF_PRESETS);
  fillPreset($('pipPreset'), PIP_PRESETS);
  await loadSession();
  loadHealth();
  if (!session.loopback && !session.paired) {
    // The pairing overlay is up; do not fire authenticated requests.
    for (const p of PAGES) $('page-' + p).classList.toggle('active', p === currentPage());
    return;
  }
  await Promise.all([loadEngines(), loadProfiles()]);
  await restoreSynthDraft();
  showPage(currentPage());
}

wireDraftPersistence();
wireSamplesStepper();
boot();
