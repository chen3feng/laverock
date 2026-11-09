// ---------------------------------------------------------------- convert
//
// Voice conversion: a source clip plus a target voice from the library.
// The request is a background job, so a long conversion keeps its progress
// across a page reload and the result is also a downloadable artifact.

const convertCapture = new Capture($('convertSource'), {
  autoTranscribe: false,
  transcribe: false,
});

let convertPreviewAudio = null;
let convertPreviewId = null;

function fillConvertProfiles() {
  const sel = $('convertProfile');
  if (!sel) return;
  const current = sel.value;
  sel.innerHTML = '';
  const empty = document.createElement('option');
  empty.value = '';
  empty.textContent = `— ${t('convert.choose_voice')} —`;
  sel.appendChild(empty);
  for (const p of profiles) {
    const o = document.createElement('option');
    o.value = p.id;
    o.textContent = p.name;
    sel.appendChild(o);
  }
  if (profiles.some((p) => p.id === current)) sel.value = current;
  syncConvertProfileUI();
}

function syncConvertProfileUI() {
  const id = $('convertProfile').value;
  const playing = !!id && convertPreviewId === id;
  $('convertPreview').disabled = !id;
  $('convertPreview').textContent = playing ? t('stop') : t('preview');
}

function stopConvertPreview() {
  if (convertPreviewAudio) {
    convertPreviewAudio.pause();
    convertPreviewAudio = null;
  }
  convertPreviewId = null;
  syncConvertProfileUI();
}

$('convertProfile').onchange = () => {
  stopConvertPreview();
  syncConvertProfileUI();
};

$('convertPreview').onclick = () => {
  const id = $('convertProfile').value;
  if (!id) return;
  if (convertPreviewId === id) {
    stopConvertPreview();
    return;
  }
  stopConvertPreview();
  const engine = engines.find((x) => x.id === $('convertEngine').value);
  const url = engine
    ? `/api/profiles/${encodeURIComponent(id)}/audio?engine=${encodeURIComponent(engine.id)}`
    : `/api/profiles/${encodeURIComponent(id)}/audio`;
  convertPreviewId = id;
  convertPreviewAudio = new Audio(url);
  convertPreviewAudio.addEventListener('ended', stopConvertPreview);
  pauseOtherAudio(convertPreviewAudio);
  convertPreviewAudio.play().catch(() => stopConvertPreview());
  syncConvertProfileUI();
};

function syncConvertEngine() {
  const e = engines.find((x) => x.id === $('convertEngine').value);
  // Re-rendering rebuilds the controls with manifest defaults, so only do
  // it when the engine actually changed (revisiting the page keeps values).
  const sig = e ? e.id : '';
  if (sig === syncConvertEngine.sig && $('convertParams').children.length) return;
  syncConvertEngine.sig = sig;
  if (!e) {
    $('convertEngineHint').textContent = t('convert.engine.hint');
    renderParamGrid($('convertParams'), [], 'convert-param-');
    return;
  }
  const caps = (e.capabilities || []).map(capabilityLabel).join(t('ui.text'));
  $('convertEngineHint').textContent = `${e.kind} · ${caps}`;
  renderParamGrid($('convertParams'), e.parameters || [], 'convert-param-');
}
$('convertEngine').onchange = syncConvertEngine;

// ---------------------------------------------------------- convert draft
// The engine, target voice and options survive reloads; the source clip
// does not (a Blob cannot be persisted), so it is chosen again.
const CONVERT_DRAFT_KEY = 'laverock.convertDraft';

function readConvertDraft() {
  try { return JSON.parse(localStorage.getItem(CONVERT_DRAFT_KEY)) || {}; } catch (_) { return {}; }
}

let convertDraftScheduled = false;
function saveConvertDraft() {
  if (convertDraftScheduled) return;
  convertDraftScheduled = true;
  setTimeout(() => {
    convertDraftScheduled = false;
    try {
      localStorage.setItem(CONVERT_DRAFT_KEY, JSON.stringify({
        engine: $('convertEngine').value,
        profile: $('convertProfile').value,
        params: collectParams('convertParams'),
      }));
    } catch (_) {}
  }, 100);
}

function restoreConvertDraft() {
  const d = readConvertDraft();
  if (!d || !Object.keys(d).length) return;
  if (d.engine && d.engine !== $('convertEngine').value &&
      engines.some((e) => e.id === d.engine)) {
    $('convertEngine').value = d.engine;
    syncConvertEngine();
  }
  if (d.profile && profiles.some((p) => p.id === d.profile)) {
    $('convertProfile').value = d.profile;
  }
  syncConvertProfileUI();
  applyDraftParams('convertParams', d.params);
}

$('convertEngine').addEventListener('change', saveConvertDraft);
$('convertProfile').addEventListener('change', saveConvertDraft);
$('convertParams').addEventListener('input', saveConvertDraft);
$('convertParams').addEventListener('change', saveConvertDraft);

function convertBlobFromEvent(p) {
  const bytes = Uint8Array.from(atob(p.wav), (c) => c.charCodeAt(0));
  return new Blob([bytes], { type: 'audio/wav' });
}

$('convertGo').onclick = async () => {
  const status = $('convertStatus');
  const engine = engines.find((x) => x.id === $('convertEngine').value);
  if (!engine) {
    status.className = 'err';
    status.textContent = t('convert.no_engine');
    return;
  }
  if (!convertCapture.blob) {
    status.className = 'err';
    status.textContent = t('convert.no_source');
    return;
  }
  const profileId = $('convertProfile').value;
  if (!profileId) {
    status.className = 'err';
    status.textContent = t('convert.no_voice');
    return;
  }

  const fd = new FormData();
  fd.append('audio', convertCapture.blob, convertCapture.filename || 'source.wav');
  fd.append('profile_id', profileId);
  for (const [name, value] of Object.entries(collectParams('convertParams'))) {
    fd.append(name, value);
  }

  $('convertGo').disabled = true;
  status.className = '';
  const labelEl = $('convertProgressLabel');
  const pbar = $('convertProgress');
  pbar.hidden = false;
  labelEl.hidden = false;
  const startedAt = Date.now();
  const fmtElapsed = () => {
    const s = (Date.now() - startedAt) / 1000;
    return `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`;
  };
  const setStage = (text) => {
    labelEl.querySelector('span').textContent = text;
    labelEl.querySelector('b').textContent = fmtElapsed();
    status.textContent = text;
  };
  setStage(engine.loaded ? t('convert.running') : t('convert.first_load'));
  const timer = setInterval(() => {
    labelEl.querySelector('b').textContent = fmtElapsed();
  }, 250);

  let result = null;
  try {
    await runJob(`/api/engines/${encodeURIComponent(engine.id)}/convert`, fd, {
      stage: (e) => setStage(e.message || t('convert.running')),
      audio: (e) => {
        result = { blob: convertBlobFromEvent(e), index: 0, total: 1 };
      },
    });
    if (!result) throw new Error(t('ui.no_audio_data_received'));
    renderSynthResults([result], engine, 'convertResults');
    loadRecentResults('vc', 'convertRecent', 'convertRecentTitle');
    lastSynthBlob = result.blob;
    engine.loaded = true;
    status.className = 'ok';
    status.textContent = t('convert.done');
  } catch (e) {
    status.className = 'err';
    status.textContent = t('convert.failed') + e.message;
  } finally {
    clearInterval(timer);
    pbar.hidden = true;
    labelEl.hidden = true;
    $('convertGo').disabled = false;
  }
};
