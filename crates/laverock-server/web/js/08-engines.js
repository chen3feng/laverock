// ---------------------------------------------------------------- engines

async function loadEngines() {
  try {
    engines = await api('/api/engines');
  } catch (e) {
    toast((t("ui.failed_to_load_engines")) + e.message, true);
    return;
  }
  const sel = $('engine');
  const current = sel.value;
  sel.innerHTML = '';
  if (!engines.length) {
    const o = document.createElement('option');
    o.textContent = t("ui.no_models_found_download_one");
    sel.appendChild(o);
    $('go').disabled = true;
  } else {
    $('go').disabled = false;
    for (const e of engines) {
      const o = document.createElement('option');
      o.value = e.id;
      o.textContent = e.name;
      sel.appendChild(o);
    }
    if (engines.some((e) => e.id === current)) sel.value = current;
  }
  syncEngine();
  if (currentPage() === 'models') {
    if (modelsData) renderModels();
    else if (!session.loopback) renderInstalledOnly();
  }
}

// Build tuning controls from the engine manifest's parameter list.
// The backend declares name/label/type/range/default/hint, so new engines
// and custom manifests need no UI changes.
function renderEngineParams(e) {
  const host = $('engineParams');
  host.innerHTML = '';
  const specs = (e && e.parameters) || [];
  host.hidden = specs.length === 0;
  if (!specs.length) return;

  host.appendChild(Object.assign(document.createElement('label'), {
    textContent: t("ui.engine_parameters"),
  }));
  const grid = document.createElement('div');
  grid.className = 'row';
  grid.style.marginTop = '10px';
  for (const p of specs) {
    const wrap = document.createElement('div');
    wrap.appendChild(paramControl(p));
    const hintText = paramHint(p);
    if (hintText) {
      const h = document.createElement('div');
      h.className = 'hint';
      h.textContent = hintText;
      wrap.appendChild(h);
    }
    grid.appendChild(wrap);
  }
  host.appendChild(grid);
}

function fmtParamValue(p, v) {
  if (v === null || v === undefined) return '';
  return p.type === 'float' ? String(Number(v)) : String(v);
}

function paramLabel(p) {
  return loc(p.label) || p.name;
}

function paramHint(p) {
  return loc(p.hint);
}

function paramOptionLabel(option) {
  return loc(option.label) || option.value;
}

function paramControl(p) {
  const id = 'param-' + p.name;
  const min = Number(p.min), max = Number(p.max), step = p.step != null ? Number(p.step) : null;
  const wideRange = p.type === 'int' && (min < 0 || max - min > 1000);

  const box = document.createElement('div');
  const lab = document.createElement('label');
  lab.htmlFor = id;
  lab.textContent = paramLabel(p);
  box.appendChild(lab);

  let input;
  if (p.type === 'bool') {
    input = document.createElement('input');
    input.type = 'checkbox';
    input.checked = !!p.default;
  } else if (p.type === 'choice') {
    input = document.createElement('select');
    for (const o of p.options || []) {
      const opt = document.createElement('option');
      opt.value = o.value;
      opt.textContent = paramOptionLabel(o);
      if (o.value === String(p.default)) opt.selected = true;
      input.appendChild(opt);
    }
  } else if (p.type === 'text' || p.type === 'text_area') {
    input = document.createElement('input');
    if (p.type === 'text_area') {
      input = document.createElement('textarea');
      input.rows = 3;
    } else {
      input.type = 'text';
    }
    input.value = p.default == null ? '' : String(p.default);
    if (p.placeholder) input.placeholder = p.placeholder;
  } else if (p.type === 'int' && wideRange) {
    input = document.createElement('input');
    input.type = 'number';
    input.value = fmtParamValue(p, p.default);
    if (p.min != null) input.min = String(p.min);
    if (p.max != null) input.max = String(p.max);
  } else {
    const row = document.createElement('div');
    row.className = 'prompt-box';
    input = document.createElement('input');
    input.type = 'range';
    input.min = p.min != null ? String(p.min) : '0';
    input.max = p.max != null ? String(p.max) : '100';
    if (step != null) input.step = String(step);
    input.value = fmtParamValue(p, p.default);
    const out = document.createElement('span');
    out.className = 'fileinfo';
    out.style.minWidth = '3.2em';
    out.textContent = input.value;
    input.addEventListener('input', () => { out.textContent = input.value; });
    row.appendChild(input);
    row.appendChild(out);
    input._outerRow = row;
  }
  input.id = id;
  input.dataset.param = p.name;
  input.dataset.type = p.type;
  if (input._outerRow) {
    box.appendChild(input._outerRow);
    delete input._outerRow;
  } else {
    box.appendChild(input);
  }
  return box;
}

function collectEngineParams() {
  const out = {};
  for (const el of document.querySelectorAll('#engineParams [data-param]')) {
    const type = el.dataset.type;
    out[el.dataset.param] = type === 'bool' ? String(el.checked) : el.value;
  }
  return out;
}

const capabilityLabel = (c) => t('cap.' + c);

function engineUsesClone(e) {
  // Preset-only engines (Kokoro) have no reference-audio cloning, so the
  // saved-voice picker and ad-hoc recorder are hidden for them.
  return !!e && (e.capabilities || []).includes('clone');
}

function syncEngine() {
  const e = engines.find((x) => x.id === $('engine').value);
  if (!e) {
    $('engineHint').textContent = '';
    renderEngineParams(null);
    return;
  }
  const caps = (e.capabilities || []).map(capabilityLabel).join(t("ui.text"));
  $('engineHint').textContent = `${e.kind} · ${(e.languages || []).join(' / ')} · ${caps}`;
  renderEngineParams(e);
  syncVisibility();
}

// Engine/voice switches must not discard text the user already typed.
$('engine').onchange = () => { syncEngine(); restoreDraftFields(); saveDraft(); };
$('profile').onchange = () => { syncProfileUI(); restoreDraftFields(); saveDraft(); };

// Rough codec-step budget for a text: CJK ~3.8 chars/s of speech,
// whitespace-split Latin words ~2.6/s; the Qwen codec emits 12.5 steps/s.
// Used only to scale the bar; it caps below 100% until the audio arrives.
function estCodecSteps(text) {
  const cjk = (text.match(/[一-鿿぀-ヿ가-힯]/g) || []).length;
  const words = text.replace(/[一-鿿぀-ヿ가-힯]/g, ' ').trim().split(/\s+/).filter(Boolean).length;
  return Math.max(40, Math.round((cjk / 3.8 + words / 2.6) * 12.5));
}

$('go').onclick = async () => {
  const status = $('status');
  const saved = selectedProfile();
  const text = $('text').value.trim();
  if (!text) { status.className = 'err'; status.textContent = t("ui.please_enter_text_to_synthesize"); return; }

  if (!saved && !synthCapture.blob && !$('engine').value) {
    status.className = 'err'; status.textContent = t("ui.please_select_an_engine");
    return;
  }

  const engine = engines.find((x) => x.id === $('engine').value);
  const isClone = engineUsesClone(engine);

  const fd = new FormData();
  fd.append('text', text);
  if (isClone) {
    if (saved) {
      fd.append('profile_id', saved.id);
    } else {
      if (synthCapture.blob) fd.append('prompt', synthCapture.blob, synthCapture.filename || 'reference.wav');
      const pt = $('prompt_text').value.trim();
      if (pt) fd.append('prompt_text', pt);
    }
  }
  for (const [name, value] of Object.entries(collectEngineParams())) {
    fd.append(name, value);
  }

  const url = `/api/engines/${encodeURIComponent($('engine').value)}/synthesize`;
  $('go').disabled = true;
  status.className = '';

  // Progress: the request is one blocking WAV response, so show stages
  // (model load vs. synthesis) with an elapsed timer rather than a fake
  // percentage. Engine load dominates the first request for each engine.
  const kind = engine ? engine.kind : '';
  const loadNote = kind === 'qwen3tts'
    ? (t("ui.first_load_takes_20_90"))
    : kind === 'longcat' ? (t("ui.first_load_takes_30_90"))
    : kind === 'voxcpm2' ? (t("ui.first_load_takes_a_few"))
    : (t("ui.first_load_takes_a_few_2"));
  const labelEl = $('synthProgressLabel');
  const pbar = $('synthProgress');
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
  const fill = pbar.querySelector('.pbar-fill');
  const setDeterminate = (pct) => {
    fill.classList.remove('indeterminate');
    fill.style.width = Math.max(0, Math.min(98, pct)).toFixed(1) + '%';
  };
  const setIndeterminate = () => {
    fill.classList.add('indeterminate');
    fill.style.width = '';
  };
  setIndeterminate();
  setStage(engine && engine.loaded
    ? (t("ui.generating_speech"))
    : t('ui.loading_model', { note: loadNote }));
  let audioText = '';
  const timer = setInterval(() => {
    labelEl.querySelector('b').textContent = fmtElapsed() + audioText;
  }, 250);
  const t0 = performance.now();
  try {
    const res = await apiFetch(url, {
      method: 'POST',
      body: fd,
      headers: { 'X-Laverock-Stream': '1' },
    });
    if (!res.ok) {
      const msg = await res.json().then((j) => j.error).catch(() => res.statusText);
      throw new Error(msg);
    }

    // Framed SSE: `event: stage|steps|wav|error` / `data: …`, blank line.
    // WAV frames carry {i, n, wav} — the take index, the batch size, and
    // the base64 audio — so a single request renders as a variant gallery.
    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buf = '';
    const variants = [];
    let lastStage = '';
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      let sep;
      while ((sep = buf.indexOf('\n\n')) !== -1) {
        const raw = buf.slice(0, sep);
        buf = buf.slice(sep + 2);
        let event = '';
        const dataLines = [];
        for (const line of raw.split('\n')) {
          if (line.startsWith('event:')) event = line.slice(6).trim();
          else if (line.startsWith('data:')) dataLines.push(line.slice(5).trim());
        }
        const data = dataLines.join('\n');
        if (event === 'stage') {
          lastStage = data;
          setStage(data + '…');
        } else if (event === 'steps') {
          const steps = Number(data);
          if (Number.isFinite(steps)) {
            if (lastStage && !lastStage.includes('合成') && !/generat/i.test(lastStage)) {
              setStage(t("ui.generating_speech"));
            }
            setDeterminate((steps / estCodecSteps(text)) * 100);
            audioText = ` · ${(steps / 12.5).toFixed(0)}s ${t("ui.audio")}`;
          }
        } else if (event === 'wav') {
          const p = JSON.parse(data);
          const bytes = Uint8Array.from(atob(p.wav), (c) => c.charCodeAt(0));
          variants[Number(p.i)] = {
            blob: new Blob([bytes], { type: 'audio/wav' }),
            index: Number(p.i),
            total: Number(p.n),
          };
        } else if (event === 'error') {
          const j = JSON.parse(data);
          throw new Error(j.message || t("ui.synthesis_failed"));
        }
      }
    }
    const got = variants.filter((v) => v && v.blob).sort((a, b) => a.index - b.index);
    if (!got.length) throw new Error(t("ui.no_audio_data_received"));
    setDeterminate(100);
    renderSynthResults(got, engine);
    // Keep the last take reachable for any downstream helper (e.g. the
    // previous top-level MP3 conversion; the gallery links are the UI).
    lastSynthBlob = got[got.length - 1].blob;
    if (engine) engine.loaded = true;
    status.className = 'ok';
    const totalBytes = got.reduce((s, v) => s + v.blob.size, 0);
    status.textContent =
      `${t("ui.done")} · ${got.length > 1 ? got.length + '× · ' : ''}` +
      `${(totalBytes / 1024).toFixed(0)} KB · ${((performance.now() - t0) / 1000).toFixed(1)}s`;
  } catch (e) {
    status.className = 'err';
    status.textContent = (t("ui.failed")) + e.message;
  } finally {
    clearInterval(timer);
    pbar.hidden = true;
    labelEl.hidden = true;
    $('go').disabled = false;
  }
};
