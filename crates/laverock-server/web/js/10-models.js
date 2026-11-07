// ---------------------------------------------------------------- models

// Cards remember whether the user expanded them so the install poll can
// re-render the list without collapsing everything.
const expandedModels = new Set();

// Cards are grouped by what the model does. The server sends `task`; the
// capability fallback keeps older remote servers and cached bundles
// working.
const MODEL_TASK_ORDER = ['tts', 'music', 'asr', 'process', 'design', 'vc'];

// Tasks that already have a workspace. Installed models get a button that
// opens it; other tasks are still listed and managed here.
const MODEL_TASK_PAGE = { tts: 'synthesize', music: 'music', process: 'captures' };

function modelTask(model) {
  if (model.task) return model.task;
  const caps = model.capabilities || [];
  if (caps.includes('music')) return 'music';
  if (caps.includes('stt')) return 'asr';
  if (caps.includes('enhance')) return 'process';
  return 'tts';
}

// Manual installs have a manifest but no catalog entry; give them the
// same shape so one card renderer serves both.
function engineAsModel(engine) {
  return {
    id: engine.id,
    dir: engine.id,
    name: engine.name,
    description: '',
    size: '',
    languages: engine.languages || [],
    capabilities: engine.capabilities || [],
    task: engine.task,
    note: null,
    unsupported: null,
    installed: true,
    loaded: !!engine.loaded,
    resident: !!engine.resident,
    memory_bytes: engine.memory_bytes ?? null,
    problem: null,
    engine_id: engine.id,
    job: null,
    manual: true,
  };
}

// Remote clients only know the engines the server reports.
function renderInstalledEngines() {
  renderModelGroups(engines.map((e) => ({ model: engineAsModel(e), installed: true })), true);
}

async function loadModels() {
  if (!session.loopback) {
    $('modelPaths').hidden = true;
    $('modelsNotice').hidden = false;
    renderInstalledEngines();
    return;
  }
  $('modelsNotice').hidden = true;
  try {
    modelsData = await api(`/api/models?lang=${lang}`);
  } catch (e) {
    toast((t("ui.failed_to_load_models")) + e.message, true);
    return;
  }
  $('modelPaths').hidden = false;
  $('modelsDirPath').textContent = modelsData.models_dir || '—';
  $('scriptsDirPath').textContent = modelsData.scripts_dir
    || (t("ui.scripts_directory_not_found_installs"));
  renderModels();
  schedulePoll();
}

function engineFor(model) {
  return engines.find((e) => e.id === (model.engine_id || model.dir));
}

// The models list carries the live state; the installed-only view used for
// remote clients falls back to the engine registry it already has.
function modelLoaded(model, engine) {
  return model.loaded !== undefined ? !!model.loaded : !!(engine && engine.loaded);
}

function loadedLabel(model, engine) {
  if (!modelLoaded(model, engine)) return t('models.unloaded');
  return model.memory_bytes
    ? t('models.loaded_state', { size: fmtBytes(model.memory_bytes) })
    : t('models.loaded');
}

function renderModels() {
  const known = new Set();
  const entries = [];
  for (const model of modelsData.models) {
    known.add(model.engine_id || model.dir);
    entries.push({ model, installed: model.installed });
  }

  // Engines whose manifest is not in the catalog (manual installs).
  for (const engine of engines) {
    if (known.has(engine.id)) continue;
    if (modelsData.models.some((m) => m.dir === engine.id)) continue;
    entries.push({ model: engineAsModel(engine), installed: true });
  }

  renderModelGroups(entries, false);
}

function renderModelGroups(entries, readonly) {
  const box = $('modelGroups');
  box.innerHTML = '';

  const groups = new Map();
  for (const entry of entries) {
    const key = modelTask(entry.model);
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(entry);
  }

  let shown = 0;
  for (const key of [...MODEL_TASK_ORDER, 'other']) {
    const items = groups.get(key);
    if (!items || items.length === 0) continue;
    // Installed models first; everything else keeps catalog order.
    items.sort((a, b) => Number(b.installed) - Number(a.installed));
    const title = document.createElement('h2');
    title.className = 'section-title';
    title.textContent = t('models.task.' + key);
    box.appendChild(title);
    const grid = document.createElement('div');
    grid.className = 'grid';
    for (const entry of items) {
      grid.appendChild(modelCard(entry.model, entry.installed, readonly));
    }
    box.appendChild(grid);
    shown += items.length;
  }

  const empty = $('installedEmpty');
  empty.textContent = t(readonly ? 'models.installed.empty' : 'models.empty');
  empty.hidden = shown > 0;
}

function progressBarHtml(job) {
  const pct = typeof job.progress === 'number' ? Math.max(0, Math.min(100, job.progress)) : null;
  const fallback = pct === null
    ? (t("ui.preparing"))
    : (t("ui.downloading"));
  const label = esc(job.stage || fallback);
  const right = pct === null ? '' : `<b>${pct.toFixed(1)}%</b>`;
  const fill = pct === null
    ? '<div class="pbar-fill indeterminate"></div>'
    : `<div class="pbar-fill" style="width:${pct.toFixed(1)}%"></div>`;
  return `<div class="pbar">${fill}</div><div class="pbar-label"><span>${label}</span>${right}</div>`;
}

function modelCard(model, installed, readonly) {
  const job = model.job;
  const running = !!job && job.status === 'running';
  const stale = installed && !!model.problem;
  const failed = !!job && job.status === 'failed' && !installed;
  const engine = engineFor(model);
  const open = expandedModels.has(model.id);
  const caps = (model.capabilities || []).map(capabilityLabel).join(t("ui.text"));
  const summary = [model.size, caps].filter(Boolean).join(t("ui.text"));
  const toggleTitle = open ? t('models.hide_details') : t('models.show_details');
  const el = document.createElement('div');
  el.className = 'model-card';
  el.innerHTML = `
    <button class="model-toggle" type="button" aria-expanded="${open}" title="${esc(toggleTitle)}">
      <span class="model-head">
        <b>${esc(model.name)}</b>
        ${installed
          ? `<span class="pill ${stale ? 'warn' : 'ok'}">${stale
            ? (t("ui.needs_update"))
            : t('models.installed')}</span>`
          : ''}
        <span class="chev" aria-hidden="true"></span>
      </span>
      ${summary ? `<span class="model-summary">${esc(summary)}</span>` : ''}
    </button>
    <div class="model-details"${open ? '' : ' hidden'}>
      ${model.description ? `<div class="model-desc">${esc(model.description)}</div>` : ''}
      ${(model.languages || []).length
        ? `<div class="chips">${(model.languages || []).map((l) => `<span class="chip">${esc(l)}</span>`).join('')}</div>`
        : ''}
      ${model.note ? `<div class="hint">${esc(model.note)}</div>` : ''}
      ${model.unsupported ? `<div class="hint err">${esc(model.unsupported)}</div>` : ''}
      ${stale ? `<div class="hint err">${esc(model.problem)}</div>` : ''}
      ${model.manual ? `<div class="hint">${t("ui.manually_installed_manifest")}</div>` : ''}
      ${installed && !stale ? `<div class="hint">${engine ? esc(engine.kind) + ' · ' : ''}${esc(loadedLabel(model, engine))}</div>` : ''}
      ${installed ? `<div class="hint">${t("ui.directory")}: models/${esc(model.dir)}</div>` : ''}
      ${job && (running || failed) ? `<pre class="log">${esc(job.log || '')}</pre>` : ''}
    </div>
    ${running ? progressBarHtml(job) : ''}
    <div class="model-actions"></div>`;

  const toggle = el.querySelector('.model-toggle');
  const details = el.querySelector('.model-details');
  toggle.onclick = () => {
    const next = toggle.getAttribute('aria-expanded') !== 'true';
    toggle.setAttribute('aria-expanded', String(next));
    toggle.title = next ? t('models.hide_details') : t('models.show_details');
    details.hidden = !next;
    if (next) expandedModels.add(model.id);
    else expandedModels.delete(model.id);
  };

  if (readonly) {
    el.querySelector('.model-actions').remove();
    return el;
  }

  const actions = el.querySelector('.model-actions');
  if (installed && !stale) {
    if (MODEL_TASK_PAGE[modelTask(model)]) actions.appendChild(useButton(model));
    if (modelLoaded(model, engine)) actions.appendChild(unloadButton(model));
    actions.appendChild(deleteButton(model));
    return el;
  }

  const scriptsReady = !!(modelsData && modelsData.scripts_dir);
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'small';
  btn.textContent = running
    ? (t("ui.installing"))
    : stale
      ? (t("ui.reinstall"))
      : failed
        ? (t("ui.retry_install"))
        : (t("ui.download_install"));
  btn.disabled = running || !scriptsReady || !!model.unsupported;
  btn.onclick = () => installModel(model.id);
  actions.appendChild(btn);
  if (installed) actions.appendChild(deleteButton(model));
  if (!scriptsReady) {
    const hint = document.createElement('span');
    hint.className = 'hint err';
    hint.textContent = t("ui.scripts_directory_not_found");
    actions.appendChild(hint);
  }
  return el;
}

// Open the workspace that uses this model, selecting it there when the
// page has an engine picker.
function useButton(model) {
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'small';
  btn.textContent = t('models.use');
  btn.onclick = () => {
    const page = MODEL_TASK_PAGE[modelTask(model)];
    if (!page) return;
    if (page === 'synthesize' || page === 'music') {
      const sel = $(page === 'music' ? 'musicEngine' : 'engine');
      const id = model.engine_id || model.dir;
      if ([...sel.options].some((o) => o.value === id)) {
        sel.value = id;
        sel.dispatchEvent(new Event('change'));
      }
    }
    location.hash = '#/' + page;
  };
  return btn;
}

// Free the model's memory while keeping it installed on disk. The card
// refreshes through the model list, which carries the live loaded state.
function unloadButton(model) {
  const id = model.engine_id || model.dir;
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'ghost small';
  btn.textContent = t('models.unload');
  btn.onclick = async () => {
    btn.disabled = true;
    try {
      await api(`/api/engines/${encodeURIComponent(id)}/unload`, { method: 'POST' });
      await Promise.all([loadEngines(), loadModels()]);
      loadSystem();
      toast(t('models.unload_done'));
    } catch (e) {
      btn.disabled = false;
      toast(e.message, true);
    }
  };
  return btn;
}

// Deleting removes files from disk, so it goes through the same
// confirmation dialog as the other destructive actions.
function deleteButton(model) {
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'ghost danger small';
  btn.textContent = t('ui.delete_model');
  btn.onclick = async () => {
    const ok = await confirmAction(
      t('ui.confirm_delete_message', { model: model.name }),
      { title: t('ui.delete_model'), okText: t('ui.delete_model') },
    );
    if (!ok) return;
    btn.disabled = true;
    try {
      await api(`/api/models/${encodeURIComponent(model.id)}`, { method: 'DELETE' });
      toast(t('ui.model_deleted', { model: model.name }));
      await Promise.all([loadEngines(), loadModels()]);
    } catch (e) {
      btn.disabled = false;
      toast(t('ui.could_not_delete_model') + e.message, true);
    }
  };
  return btn;
}

async function installModel(id) {
  try {
    await api(`/api/models/${encodeURIComponent(id)}/install`, { method: 'POST' });
  } catch (e) {
    toast((t("ui.could_not_start_the_install")) + e.message, true);
    return;
  }
  toast(t("ui.download_started_progress_is_shown"));
  await loadModels();
  // A small or cached install can finish before the first poll, and the
  // poll is what normally refreshes the engine registry. Check once here
  // so the new engine appears in Synthesize either way.
  const model = modelsData && modelsData.models.find((m) => m.id === id);
  if (!model || !model.job || model.job.status !== 'running') {
    await loadEngines();
    loadHealth();
  }
}

function schedulePoll() {
  if (modelsTimer) {
    clearInterval(modelsTimer);
    modelsTimer = null;
  }
  if (!modelsData || !modelsData.models.some((m) => m.job && m.job.status === 'running')) return;
  modelsTimer = setInterval(async () => {
    // The install runs on the server, so keep watching even after the
    // user leaves Models: the engine list must be current the next time
    // they open Synthesize.
    if (!session.loopback) {
      clearInterval(modelsTimer);
      modelsTimer = null;
      return;
    }
    try {
      modelsData = await api(`/api/models?lang=${lang}`);
    } catch (_) {
      return;
    }
    if (currentPage() === 'models') renderModels();
    const running = modelsData.models.some((m) => m.job && m.job.status === 'running');
    if (!running) {
      clearInterval(modelsTimer);
      modelsTimer = null;
      const failed = modelsData.models.some((m) => m.job && m.job.status === 'failed' && !m.installed);
      await loadEngines();
      await loadModels();
      // An STT model install flips the transcribe health flag.
      loadHealth();
      toast(failed
        ? (t("ui.install_failed_see_the_log"))
        : (t("ui.install_finished")), failed);
    }
  }, 1500);
}
