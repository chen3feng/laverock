// ---------------------------------------------------------------- recording library

// Takes are independent of voices: any selection of them can become a
// voice, and deleting one here only removes it from the voices using it.
// A voice's transcript and warnings come from the server, which measures
// the takes when they are stored.
let captureList = [];
let selectedCaptures = new Set();
// Transcript produced inside the add-take card, if the user transcribed
// before saving; otherwise the server transcribes the stored take.
let libraryTranscript = '';
// Which enhancement stages the installed models cover.
let enhanceStatus = { denoise: false, vocals: false };

const enhanceModeAvailable = (mode) => {
  if (mode === 'denoise') return enhanceStatus.denoise;
  if (mode === 'vocals') return enhanceStatus.vocals;
  if (mode === 'both') return enhanceStatus.denoise && enhanceStatus.vocals;
  return true;
};

const enhanceModeLabel = (mode) => t('captures.enhance.' + (mode || 'off'));

async function loadEnhanceStatus() {
  try {
    enhanceStatus = await api('/api/enhance');
  } catch (_) {
    enhanceStatus = { denoise: false, vocals: false };
  }
  syncEnhanceSelect();
}

// Grey out modes whose models are missing and explain how to get them.
// Denoising is the sensible default: it is fast and only removes noise,
// so the user does not have to pick anything. The last used mode wins,
// including an explicit "off"; a mode whose models disappeared falls
// back to the default.
function syncEnhanceSelect() {
  const select = $('captureEnhance');
  if (!select) return;
  for (const opt of select.options) {
    opt.disabled = !enhanceModeAvailable(opt.value);
  }
  const saved = localStorage.getItem('laverock.enhance');
  const preferred = saved ?? (enhanceStatus.denoise ? 'denoise' : 'off');
  select.value = enhanceModeAvailable(preferred) ? preferred : 'off';
  $('captureEnhanceHint').textContent =
    enhanceStatus.denoise && enhanceStatus.vocals ? '' : t('captures.enhance.missing');
}

const captureUrl = (id) => '/api/captures/' + encodeURIComponent(id) + '/audio';

async function loadCaptures() {
  try {
    captureList = await api('/api/captures');
  } catch (e) {
    toast(t('captures.load_failed') + e.message, true);
    return;
  }
  for (const id of [...selectedCaptures]) {
    if (!captureList.some((c) => c.id === id)) selectedCaptures.delete(id);
  }
  renderCaptures();
}

function captureIssueText(issue) {
  const secs = issue.value ? issue.value.toFixed(1) : '';
  return t('captures.issue.' + issue.code).replace('{secs}', secs);
}

// How much of the selected takes the chosen engine will actually hear.
function captureLimitText() {
  const engine = currentEngine();
  const limit = engine && engine.reference_limit_secs;
  if (!limit) return '';
  const total = captureList
    .filter((c) => selectedCaptures.has(c.id))
    .reduce((sum, c) => sum + (c.duration_secs || 0), 0);
  if (total <= limit) return '';
  return t('captures.limit').replace('{limit}', limit.toFixed(0));
}

function syncCaptureBar() {
  const n = selectedCaptures.size;
  $('captureBar').hidden = n === 0;
  if (!n) return;
  $('captureSelected').textContent = t('captures.selected', { n });
  $('captureLimit').textContent = captureLimitText();
}

function renderCaptures() {
  const grid = $('captureGrid');
  if (!grid) return;
  grid.innerHTML = '';
  $('capturesEmpty').hidden = captureList.length > 0;
  for (const c of captureList) {
    const el = document.createElement('div');
    el.className = 'voice-card capture-card';
    const name = c.name || t('captures.unnamed');
    const checked = selectedCaptures.has(c.id) ? ' checked' : '';
    const transcript = c.prompt_text
      ? esc(c.prompt_text)
      : `<span class="muted">${esc(t('captures.not_transcribed'))}</span>`;
    el.innerHTML = `
      <div class="voice-top">
        <label class="cap-check"><input type="checkbox" class="pick"${checked} /></label>
        <b>${esc(name)}</b>
        ${c.duplicate_of ? `<span class="chip">${esc(t('captures.duplicate'))}</span>` : ''}
        ${c.enhancement ? `<span class="chip">${esc(t('captures.enhanced'))} · ${esc(enhanceModeLabel(c.enhancement))}</span>` : ''}
      </div>
      <div class="voice-meta">${fmtDur(c.duration_secs)} · ${fmtBytes(c.bytes)} · ${fmtDate(c.created_at)}</div>
      ${c.issues.map((i) => `<div class="voice-warn">⚠ ${esc(captureIssueText(i))}</div>`).join('')}
      <div class="voice-text">${transcript}</div>
      <div class="voice-actions">
        <button class="ghost small play" type="button">${previewId === c.id ? t('stop') : t('preview')}</button>
        <button class="ghost small tr" type="button"${transcribeAvailable ? '' : ' disabled'}>${t('captures.retranscribe')}</button>
        <button class="ghost small enh" type="button"${enhanceStatus.denoise || enhanceStatus.vocals ? '' : ' disabled'}>${t('captures.enhance.action')}</button>
        <button class="ghost small edit" type="button">${t('captures.rename')}</button>
        <button class="ghost small danger del" type="button">${t('ui.delete')}</button>
      </div>`;
    el.querySelector('.pick').onchange = (e) => {
      if (e.target.checked) selectedCaptures.add(c.id);
      else selectedCaptures.delete(c.id);
      syncCaptureBar();
    };
    el.querySelector('.play').onclick = () => togglePreview(c.id, captureUrl(c.id));
    el.querySelector('.tr').onclick = () => transcribeCapture(c);
    el.querySelector('.enh').onclick = () => enhanceCapture(c);
    el.querySelector('.edit').onclick = () => editCapture(c);
    el.querySelector('.del').onclick = () => deleteCapture(c);
    grid.appendChild(el);
  }
  syncCaptureBar();
}

async function transcribeCapture(c) {
  if (!transcribeAvailable) {
    toast(t('ui.install_the_sensevoice_speech_to'), true);
    return;
  }
  toast(t('captures.transcribing'));
  try {
    await api('/api/captures/' + encodeURIComponent(c.id) + '/transcribe', { method: 'POST' });
    toast(t('captures.transcribed'));
  } catch (e) {
    toast(t('ui.transcription_failed') + e.message, true);
  }
  // Voices built from this take got a new transcript too.
  await Promise.all([loadCaptures(), loadProfiles()]);
}

// Apply or revert enhancement on a stored take. The server always starts
// from the untouched upload, so switching modes never compounds; `off`
// restores the trimmed original.
async function enhanceCapture(c) {
  const options = [{ value: 'off', label: t('captures.enhance.off') }];
  if (enhanceStatus.denoise) {
    options.push({ value: 'denoise', label: t('captures.enhance.denoise') });
  }
  if (enhanceStatus.vocals) {
    options.push({ value: 'vocals', label: t('captures.enhance.vocals') });
  }
  if (enhanceStatus.denoise && enhanceStatus.vocals) {
    options.push({ value: 'both', label: t('captures.enhance.both') });
  }
  const fields = [{
    name: 'mode',
    label: t('captures.enhance'),
    type: 'select',
    options,
    value: c.enhancement || 'off',
  }];
  // An enhanced take keeps its upload, so the dialog can play both
  // versions before the user commits to another mode.
  if (c.enhancement) {
    fields.push({
      name: 'preview',
      label: t('captures.enhance.preview'),
      hint: t('captures.enhance.preview.hint'),
      type: 'audio',
      options: [
        { label: t('captures.enhance.preview.original'), url: captureUrl(c.id) + '?original=1' },
        { label: t('captures.enhance.preview.processed'), url: captureUrl(c.id) },
      ],
    });
  }
  const res = await showDialog({
    title: t('captures.enhance.action'),
    message: t('captures.enhance.dialog'),
    okText: t('ui.save'),
    fields,
  });
  if (!res) return;
  const mode = res.values.mode || 'off';
  if (mode === 'vocals' || mode === 'both') toast(t('captures.enhancing'));
  try {
    await api('/api/captures/' + encodeURIComponent(c.id) + '/enhance', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ mode }),
    });
    toast(t('captures.enhanced.done'));
  } catch (e) {
    toast(t('ui.failed') + e.message, true);
  }
  await loadCaptures();
}

async function editCapture(c) {
  const res = await showDialog({
    title: t('captures.rename'),
    okText: t('ui.save'),
    fields: [
      { name: 'name', label: t('ui.name'), type: 'text', value: c.name },
      {
        name: 'prompt_text',
        label: t('ui.reference_transcript'),
        type: 'textarea',
        value: c.prompt_text,
        hint: t('ui.required_for_zipvoice_may_be'),
      },
    ],
  });
  if (!res) return;
  const fd = new FormData();
  fd.append('name', res.values.name || c.name);
  fd.append('prompt_text', res.values.prompt_text);
  try {
    await api('/api/captures/' + encodeURIComponent(c.id), { method: 'POST', body: fd });
    await Promise.all([loadCaptures(), loadProfiles()]);
  } catch (e) {
    toast(t('ui.failed') + e.message, true);
  }
}

async function deleteCapture(c) {
  const name = c.name || t('captures.unnamed');
  if (!(await confirmAction(t('captures.delete_confirm').replace('{name}', name)))) return;
  try {
    await api('/api/captures/' + encodeURIComponent(c.id), { method: 'DELETE' });
    toast(t('captures.deleted'));
  } catch (e) {
    toast(t('ui.failed') + e.message, true);
  }
  await Promise.all([loadCaptures(), loadProfiles()]);
}

async function dedupeCaptures() {
  try {
    const report = await api('/api/captures/dedupe', { method: 'POST' });
    toast(report.removed
      ? t('captures.deduped', { n: report.removed })
      : t('captures.nothing_deduped'));
  } catch (e) {
    toast(t('ui.failed') + e.message, true);
  }
  await Promise.all([loadCaptures(), loadProfiles()]);
}

// Add the clip recorded or picked in the card as a take, transcribing it
// on the server when nothing did so in the browser.
async function saveCapture() {
  if (!libraryCapture.blob) {
    toast(t('ui.choose_or_record_a_reference'), true);
    return;
  }
  const fd = new FormData();
  fd.append('audio', libraryCapture.blob, libraryCapture.filename || 'take.wav');
  fd.append('name', t('captures.default_name', { n: captureList.length + 1 }));
  const mode = $('captureEnhance').value || 'off';
  if (mode !== 'off') fd.append('enhance', mode);
  localStorage.setItem('laverock.enhance', mode);
  if (libraryTranscript) fd.append('prompt_text', libraryTranscript);
  let meta;
  try {
    if (mode === 'vocals' || mode === 'both') toast(t('captures.enhancing'));
    meta = await api('/api/captures', { method: 'POST', body: fd });
  } catch (e) {
    toast(t('ui.failed') + e.message, true);
    return;
  }
  if (!libraryTranscript && transcribeAvailable) {
    try {
      await api('/api/captures/' + encodeURIComponent(meta.id) + '/transcribe', { method: 'POST' });
    } catch (_) {
      // The take is stored either way; it can be transcribed later.
    }
  }
  toast(t('captures.saved'));
  $('newCaptureCard').hidden = true;
  libraryCapture.clear();
  libraryTranscript = '';
  await loadCaptures();
}

// Turn the ticked takes into one voice, in the order the library lists
// them. The server joins the audio up to the engine's reference limit.
async function capturesToVoice() {
  const ids = captureList.filter((c) => selectedCaptures.has(c.id)).map((c) => c.id);
  if (!ids.length) {
    toast(t('captures.none_selected'), true);
    return;
  }
  const first = captureList.find((c) => c.id === ids[0]);
  const res = await showDialog({
    title: t('captures.toVoice'),
    okText: t('ui.save'),
    fields: [{
      name: 'name',
      label: t('captures.voice_name'),
      type: 'text',
      value: first ? first.name : '',
      placeholder: t('captures.voice_name.ph'),
    }],
  });
  if (!res) return;
  const fd = new FormData();
  fd.append('name', res.values.name || t('captures.default_name', { n: captureList.length }));
  // Repeated fields: order is the voice's take order.
  for (const id of ids) fd.append('capture', id);
  try {
    const voice = await api('/api/profiles', { method: 'POST', body: fd });
    toast(t('captures.added_voice', { name: voice.name, n: ids.length }));
    selectedCaptures.clear();
  } catch (e) {
    toast(t('ui.failed') + e.message, true);
    return;
  }
  await Promise.all([loadCaptures(), loadProfiles()]);
}

const libraryCapture = new Capture($('libraryCapture'));
libraryCapture.onTranscribed = (text) => {
  libraryTranscript = text;
};
// A different clip invalidates the transcript from the previous one.
libraryCapture.onChange = () => {
  libraryTranscript = '';
};
if (!window.isSecureContext) {
  libraryCapture.rec.disabled = true;
  libraryCapture.rec.title = t('ui.microphone_access_requires_https_or');
}

$('newCaptureBtn').onclick = () => {
  $('newCaptureCard').hidden = false;
  $('newCaptureHint').textContent = '';
};
$('newCaptureCancel').onclick = () => {
  $('newCaptureCard').hidden = true;
  libraryCapture.clear();
  libraryTranscript = '';
};
$('newCaptureSave').onclick = saveCapture;
$('dedupeCapturesBtn').onclick = dedupeCaptures;
$('capturesToVoice').onclick = capturesToVoice;
