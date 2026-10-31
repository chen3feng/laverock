// ---------------------------------------------------------------- models

async function loadModels() {
  if (!session.loopback) {
    $('modelPaths').hidden = true;
    $('modelsNotice').hidden = false;
    $('catalogSection').hidden = true;
    renderInstalledOnly();
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

function renderInstalledOnly() {
  const box = $('installedModels');
  box.innerHTML = '';
  for (const e of engines) {
    const el = document.createElement('div');
    el.className = 'model-card';
    el.innerHTML = `
      <div class="model-head"><b>${esc(e.name)}</b><span class="pill ok">${t('models.installed')}</span></div>
      <div class="chips">${(e.languages || []).map((l) => `<span class="chip">${esc(l)}</span>`).join('')}</div>
      <div class="hint">${esc(e.kind)} · ${esc((e.capabilities || []).map(capabilityLabel).join(t("ui.text")))}</div>`;
    box.appendChild(el);
  }
  $('installedEmpty').hidden = engines.length > 0;
}

function renderModels() {
  const installedBox = $('installedModels');
  const catalogBox = $('catalogModels');
  installedBox.innerHTML = '';
  catalogBox.innerHTML = '';

  const catalogEngineIds = new Set();
  for (const model of modelsData.models) {
    if (model.installed) {
      catalogEngineIds.add(model.engine_id || model.dir);
      installedBox.appendChild(modelCard(model, true));
    } else {
      catalogBox.appendChild(modelCard(model, false));
    }
  }

  // Engines whose manifest is not in the catalog (manual installs).
  for (const engine of engines) {
    if (catalogEngineIds.has(engine.id)) continue;
    if (modelsData.models.some((m) => m.dir === engine.id)) continue;
    const el = document.createElement('div');
    el.className = 'model-card';
    el.innerHTML = `
      <div class="model-head"><b>${esc(engine.name)}</b><span class="pill ok">${t('models.installed')}</span></div>
      <div class="chips">${(engine.languages || []).map((l) => `<span class="chip">${esc(l)}</span>`).join('')}</div>
      <div class="hint">${esc(engine.kind)} · ${t("ui.manually_installed_manifest")}</div>`;
    installedBox.appendChild(el);
  }

  $('installedEmpty').hidden = installedBox.children.length > 0;
  $('catalogSection').hidden = catalogBox.children.length === 0;
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

function modelCard(model, installed) {
  const job = model.job;
  const running = !!job && job.status === 'running';
  const stale = installed && !!model.problem;
  const failed = !!job && job.status === 'failed' && !installed;
  const engine = engineFor(model);
  const el = document.createElement('div');
  el.className = 'model-card';
  el.innerHTML = `
    <div class="model-head">
      <b>${esc(model.name)}</b>
      ${installed
        ? `<span class="pill ${stale ? 'warn' : 'ok'}">${stale
          ? (t("ui.needs_update"))
          : t('models.installed')}</span>`
        : `<span class="pill">${esc(model.size || '')}</span>`}
    </div>
    <div class="model-desc">${esc(model.description)}</div>
    <div class="chips">${(model.languages || []).map((l) => `<span class="chip">${esc(l)}</span>`).join('')}</div>
    ${model.note ? `<div class="hint">${esc(model.note)}</div>` : ''}
    ${model.unsupported ? `<div class="hint err">${esc(model.unsupported)}</div>` : ''}
    ${stale ? `<div class="hint err">${esc(model.problem)}</div>` : ''}
    ${installed && !stale && engine ? `<div class="hint">${esc(engine.kind)} · ${engine.loaded
      ? (t("ui.loaded_in_memory"))
      : (t("ui.loaded_on_first_use"))}</div>` : ''}
    ${running ? progressBarHtml(job) : ''}
    ${job && (running || failed) ? `<pre class="log">${esc(job.log || '')}</pre>` : ''}
    <div class="model-actions"></div>`;

  const actions = el.querySelector('.model-actions');
  if (installed && !stale) {
    actions.innerHTML = `<span class="hint">${t("ui.directory")}: models/${esc(model.dir)}</span>`;
    actions.appendChild(deleteButton(model));
    return el;
  }

  const btn = document.createElement('button');
  btn.type = 'button';
  btn.textContent = running
    ? (t("ui.installing"))
    : stale
      ? (t("ui.reinstall"))
      : failed
        ? (t("ui.retry_install"))
        : (t("ui.download_install"));
  btn.disabled = running || !modelsData.scripts_dir || !!model.unsupported;
  btn.onclick = () => installModel(model.id);
  actions.appendChild(btn);
  if (installed) {
    const dir = document.createElement('span');
    dir.className = 'hint';
    dir.textContent = `${t("ui.directory")}: models/${model.dir}`;
    actions.appendChild(dir);
    actions.appendChild(deleteButton(model));
  }
  if (!modelsData.scripts_dir) {
    const hint = document.createElement('span');
    hint.className = 'hint err';
    hint.textContent = t("ui.scripts_directory_not_found");
    actions.appendChild(hint);
  }
  return el;
}

// Deleting is irreversible, so the first click only arms the button; the
// second one within a few seconds actually removes the model directory.
function deleteButton(model) {
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'ghost danger';
  btn.textContent = t('ui.delete_model');
  let armed = false;
  let timer = null;
  const disarm = () => {
    armed = false;
    clearTimeout(timer);
    btn.textContent = t('ui.delete_model');
  };
  btn.onclick = async () => {
    if (!armed) {
      armed = true;
      btn.textContent = t('ui.confirm_delete_model');
      timer = setTimeout(disarm, 4000);
      return;
    }
    disarm();
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
}

function schedulePoll() {
  if (modelsTimer) {
    clearInterval(modelsTimer);
    modelsTimer = null;
  }
  if (!modelsData || !modelsData.models.some((m) => m.job && m.job.status === 'running')) return;
  modelsTimer = setInterval(async () => {
    if (currentPage() !== 'models' || !session.loopback) {
      clearInterval(modelsTimer);
      modelsTimer = null;
      return;
    }
    try {
      modelsData = await api(`/api/models?lang=${lang}`);
    } catch (_) {
      return;
    }
    renderModels();
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
