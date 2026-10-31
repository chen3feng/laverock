// ---------------------------------------------------------------- synth draft
// The synth form is a long-lived draft: WKWebView can drop textarea
// contents around the native <select> popover (and a reload wipes the
// whole page), so text/choices are saved continuously and restored after
// any engine/voice switch or page load.

const DRAFT_KEY = 'laverock.synthDraft';

function readDraft() {
  try { return JSON.parse(localStorage.getItem(DRAFT_KEY)) || {}; } catch (_) { return {}; }
}

function collectDraft() {
  const params = {};
  document.querySelectorAll('#engineParams [data-param]').forEach((el) => {
    params[el.dataset.param] = el.value;
  });
  return {
    engine: $('engine').value,
    profile: $('profile').value,
    text: $('text').value,
    prompt: $('prompt_text').value,
    params,
  };
}

let draftScheduled = false;
function saveDraft() {
  // Coalesce bursts of input events.
  if (draftScheduled) return;
  draftScheduled = true;
  setTimeout(() => {
    draftScheduled = false;
    localStorage.setItem(DRAFT_KEY, JSON.stringify(collectDraft()));
  }, 100);
}

// Re-apply saved text/transcript fields. Called after selects change, so
// engine/voice switches can never discard text the user already typed.
function restoreDraftFields() {
  const d = readDraft();
  if (typeof d.text === 'string') $('text').value = d.text;
  if (typeof d.prompt === 'string') $('prompt_text').value = d.prompt;
}

function wireSamplesStepper() {
  const input = $('samples');
  const clamp = () => {
    let n = Math.round(Number(input.value));
    if (!Number.isFinite(n)) n = 1;
    input.value = Math.max(1, Math.min(4, n));
  };
  input.addEventListener('change', clamp);
  $('samplesMinus').onclick = () => { input.value = Math.max(1, Number(input.value) - 1); };
  $('samplesPlus').onclick = () => { input.value = Math.min(4, Number(input.value) + 1); };
}

function wireDraftPersistence() {
  $('text').addEventListener('input', saveDraft);
  $('prompt_text').addEventListener('input', saveDraft);
  document.addEventListener('input', (e) => {
    if (e.target.closest && e.target.closest('#engineParams')) saveDraft();
  });
  document.addEventListener('change', (e) => {
    if (['engine', 'profile'].includes(e.target.id) ||
        (e.target.closest && e.target.closest('#engineParams'))) {
      saveDraft();
    }
  });
}

// Full restore after boot: engine first (rebuilds parameter controls),
// then the saved voice, parameter values and text.
async function restoreSynthDraft() {
  const d = readDraft();
  if (!d || !Object.keys(d).length) return;
  if (d.engine && d.engine !== $('engine').value &&
      engines.some((e) => e.id === d.engine)) {
    $('engine').value = d.engine;
    syncEngine();
  }
  if (d.profile && profiles.some((p) => p.id === d.profile)) {
    $('profile').value = d.profile;
  }
  syncProfileUI();
  if (d.params) {
    for (const [name, value] of Object.entries(d.params)) {
      const el = document.querySelector(`#engineParams [data-param="${name}"]`);
      if (!el) continue;
      // Selects only accept one of their option values; range/text
      // inputs take the raw value.
      const acceptable = !el.options || [...el.options].some((o) => o.value === value);
      if (acceptable) el.value = value;
    }
  }
  restoreDraftFields();
}

async function loadProfiles() {
  try {
    profiles = await api('/api/profiles');
  } catch (e) {
    toast((t("ui.failed_to_load_voices")) + e.message, true);
    return;
  }
  const sel = $('profile');
  const current = sel.value;
  sel.innerHTML = `<option value="">— ${t("ui.temporary_recording_upload")} —</option>`;
  for (const p of profiles) {
    const o = document.createElement('option');
    o.value = p.id;
    o.textContent = p.name;
    sel.appendChild(o);
  }
  if (profiles.some((p) => p.id === current)) sel.value = current;
  syncProfileUI();
  renderVoices();
}

function voiceWarnings(p) {
  const r = p.reference;
  if (!r) return [];
  const out = [];
  if (r.clipped_fraction > 0.001) out.push(t('voice.warn.clipped'));
  if (r.longest_pause_secs >= 0.8) {
    out.push(t('voice.warn.pause').replace('{secs}', r.longest_pause_secs.toFixed(1)));
  }
  if (r.duration_secs > 0 && r.duration_secs < 1.5) out.push(t('voice.warn.short'));
  else if (r.duration_secs > 15) out.push(t('voice.warn.long'));
  if (r.peak > 0 && r.peak < 0.05 && r.clipped_fraction <= 0.001) out.push(t('voice.warn.quiet'));
  return out;
}

// A voice previews what the selected engine would hear, so the engine
// travels with the request and its reference limit applies.
function profilePreviewUrl(id) {
  const engine = currentEngine();
  const url = `/api/profiles/${encodeURIComponent(id)}/audio`;
  return engine ? `${url}?engine=${encodeURIComponent(engine.id)}` : url;
}

function renderVoices() {
  const grid = $('voicesGrid');
  grid.innerHTML = '';
  $('voicesEmpty').hidden = profiles.length > 0;
  for (const p of profiles) {
    const el = document.createElement('div');
    el.className = 'voice-card';
    const warns = voiceWarnings(p);
    // A voice is engine-agnostic: the engine is picked at synthesis, so
    // the engine it was first saved with is not shown as a binding.
    el.innerHTML = `
      <div class="voice-top"><b>${esc(p.name)}</b>${p.capture_count > 1 ? `<span class="chip">${esc(t('voices.takes', { n: p.capture_count }))}</span>` : ''}</div>
      <div class="voice-meta">${fmtDur(p.duration_secs)} · ${fmtBytes(p.bytes)} · ${fmtDate(p.created_at)}</div>
      ${warns.map((w) => `<div class="voice-warn">⚠ ${esc(w)}</div>`).join('')}
      <div class="voice-text">${esc(p.prompt_text)}</div>
      <div class="voice-actions">
        <button class="ghost small play" type="button">${previewId === p.id ? t('stop') : t('preview')}</button>
        <button class="ghost small edit" type="button">${t("ui.edit")}</button>
        <button class="ghost small export" type="button">${t("ui.export")}</button>
        <button class="ghost small danger del" type="button">${t("ui.delete")}</button>
      </div>`;
    el.querySelector('.play').onclick = () => togglePreview(p.id, profilePreviewUrl(p.id));
    el.querySelector('.edit').onclick = () => editProfile(p);
    el.querySelector('.export').onclick = () => exportProfile(p);
    el.querySelector('.del').onclick = () => deleteProfile(p);
    grid.appendChild(el);
  }
}

// `key` identifies what is playing (a voice or a take id); `url` is what
// to play. A voice previews the clip the selected engine would hear, so
// the engine's reference limit applies here too.
function togglePreview(key, url) {
  const audio = $('previewer');
  if (previewId === key) {
    audio.pause();
    audio.removeAttribute('src');
    previewId = null;
  } else {
    previewId = key;
    pauseOtherAudio(audio);
    audio.src = url;
    audio.play().catch(() => {});
  }
  renderVoices();
  if (typeof renderCaptures === 'function') renderCaptures();
  syncProfileUI();
}

$('previewer').addEventListener('ended', () => {
  previewId = null;
  renderVoices();
  if (typeof renderCaptures === 'function') renderCaptures();
  syncProfileUI();
});

// pauseOtherAudio calls this when it stops a running preview because a
// native player started, so the lists drop their play/stop icons too.
function resetPreviewState() {
  previewId = null;
  renderVoices();
  if (typeof renderCaptures === 'function') renderCaptures();
  syncProfileUI();
}

async function editProfile(p) {
  const res = await showDialog({
    title: t("ui.edit_voice"),
    okText: t("ui.save"),
    fields: [
      { name: 'name', label: t("ui.name"), type: 'text', value: p.name },
      {
        name: 'prompt_text',
        label: t("ui.reference_transcript"),
        type: 'textarea',
        value: p.prompt_text,
        hint: t("ui.required_for_zipvoice_may_be"),
      },
      {
        name: 'prompt',
        label: t("ui.replace_reference_clip_optional_3"),
        type: 'file',
        hint: t("ui.leave_empty_to_keep_the"),
      },
    ],
  });
  if (!res || !res.ok) return;
  const { name, prompt_text, prompt } = res.values;
  if (!name) {
    toast(t("ui.name_cannot_be_empty"), true);
    return;
  }
  const fd = new FormData();
  fd.append('name', name);
  fd.append('prompt_text', prompt_text || '');
  if (prompt) fd.append('prompt', prompt, prompt.name || 'reference.wav');
  try {
    await api(`/api/profiles/${encodeURIComponent(p.id)}`, { method: 'POST', body: fd });
    toast(t('ui.updated_voice', { name }));
    await loadProfiles();
  } catch (e) {
    toast((t("ui.save_failed")) + e.message, true);
  }
}

async function deleteProfile(p) {
  const yes = await confirmAction(
    t('ui.delete_voice_confirm', { name: p.name }),
    { title: t("ui.delete_voice"), okText: t("ui.delete") },
  );
  if (!yes) return;
  try {
    await api(`/api/profiles/${encodeURIComponent(p.id)}`, { method: 'DELETE' });
  } catch (e) {
    toast((t("ui.delete_failed")) + e.message, true);
    return;
  }
  if (previewId === p.id) {
    $('previewer').pause();
    previewId = null;
  }
  toast(t('ui.deleted_voice', { name: p.name }));
  await loadProfiles();
}

function voiceFileName(name, ext) {
  const stem = String(name || 'voice').trim().replace(/[\\/:*?"<>|]+/g, '_').replace(/\s+/g, '-') || 'voice';
  return `${stem}.${ext}`;
}

async function exportProfile(p) {
  try {
    const res = await apiFetch(`/api/profiles/${encodeURIComponent(p.id)}/export`);
    if (!res.ok) {
      const body = await res.json().catch(() => ({}));
      throw new Error(body.error || res.statusText);
    }
    await saveBlob(await res.blob(), voiceFileName(p.name, 'lvvoice.zip'));
    toast(t("ui.voice_exported"));
  } catch (e) {
    toast((t("ui.export_failed")) + e.message, true);
  }
}

async function importProfile(file, strategy) {
  const fd = new FormData();
  fd.append('file', file, file.name || 'voices.lvvoices.zip');
  if (strategy) fd.append('strategy', strategy);
  try {
    const r = await api('/api/profiles/import', { method: 'POST', body: fd });
    await loadProfiles();
    const summary = (t("ui.imported_n_skipped_s_overwritten"))
      .replace('{n}', r.imported).replace('{s}', r.skipped).replace('{o}', r.overwritten);
    if (r.failed && r.failed.length) {
      toast((t("ui.some_voices_failed")) + r.failed.join('；'), true);
    } else {
      toast(summary);
    }
    return r;
  } catch (e) {
    toast((t("ui.import_failed")) + e.message, true);
    return null;
  }
}

async function createProfile(name, promptText, blob) {
  if (!blob) throw new Error(t("ui.choose_or_record_a_reference"));
  if (!name) throw new Error(t("ui.please_name_the_voice"));
  if (!promptText) throw new Error(t("ui.please_fill_in_the_reference"));
  const fd = new FormData();
  fd.append('name', name);
  fd.append('prompt_text', promptText);
  fd.append('prompt', blob, blob.name || 'reference.wav');
  const res = await apiFetch('/api/profiles', { method: 'POST', body: fd });
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || res.statusText);
  }
  return res.json();
}

$('preview').onclick = () => {
  const p = selectedProfile();
  if (p) togglePreview(p.id, profilePreviewUrl(p.id));
};

$('saveprofile').onclick = async () => {
  const hint = $('savehint');
  hint.className = 'hint';
  try {
    const meta = await createProfile(
      $('profilename').value.trim(),
      $('prompt_text').value.trim(),
      synthCapture.blob,
    );
    $('profilename').value = '';
    $('savehint').textContent = t('ui.saved_for_picker', { name: meta.name });
    await loadProfiles();
    $('profile').value = meta.id;
    syncProfileUI();
    toast(t('ui.saved_voice', { name: meta.name }));
  } catch (e) {
    hint.className = 'hint err';
    hint.textContent = (t("ui.save_failed")) + e.message;
  }
};

$('importVoiceBtn').onclick = () => $('importVoiceFile').click();
$('importVoiceFile').onchange = async function () {
  const file = this.files[0];
  this.value = '';
  if (!file) return;
  const strategy = await chooseImportStrategy();
  if (!strategy) return; // cancelled
  await importProfile(file, strategy);
};
