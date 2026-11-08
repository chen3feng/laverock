// ---------------------------------------------------------------- settings

function fillPreset(sel, presets) {
  sel.innerHTML = presets.map((p) =>
    `<option value="${esc(p.value)}">${esc(p.labelKey ? t(p.labelKey) : p.label)}</option>`).join('')
    + `<option value="__custom">${t('preset.custom')}</option>`;
}

function setPreset(prefix, value, presets) {
  const sel = $(prefix + 'Preset');
  const custom = $(prefix + 'Custom');
  const clean = (v) => String(v || '').replace(/\/+$/, '');
  if (!value) {
    sel.value = '';
    custom.hidden = true;
    custom.value = '';
    return;
  }
  const hit = presets.find((p) => p.value && clean(p.value) === clean(value));
  if (hit) {
    sel.value = hit.value;
    custom.hidden = true;
    custom.value = '';
  } else {
    sel.value = '__custom';
    custom.hidden = false;
    custom.value = value;
  }
}

function getPreset(prefix) {
  const sel = $(prefix + 'Preset');
  return sel.value === '__custom' ? $(prefix + 'Custom').value.trim() : sel.value;
}

function wirePreset(prefix) {
  const sel = $(prefix + 'Preset');
  const custom = $(prefix + 'Custom');
  sel.onchange = () => {
    custom.hidden = sel.value !== '__custom';
    if (!custom.hidden) custom.focus();
  };
}

fillPreset($('hfPreset'), HF_PRESETS);
fillPreset($('pipPreset'), PIP_PRESETS);
wirePreset('hf');
wirePreset('pip');

let detectedProxy = null;
let systemData = null;

// Live memory figures for the loaded engines and the server itself.
async function loadSystem() {
  const card = $('systemCard');
  if (!session.loopback) {
    card.hidden = true;
    return;
  }
  card.hidden = false;
  try {
    systemData = await api('/api/system');
    $('systemRss').textContent = fmtBytes(systemData.rss);
    $('systemBackends').textContent = (systemData.backends || []).join(' · ') || t('system.none');
    const children = systemData.children || [];
    $('systemChildren').textContent = children.length
      ? children.map((c) => `${c.engine} · ${fmtBytes(c.rss)}`).join(' · ')
      : t('system.no_children');
  } catch (e) {
    $('systemHint').className = 'hint err';
    $('systemHint').textContent = e.message;
  }
}

$('unloadAll').onclick = async () => {
  const btn = $('unloadAll');
  const hint = $('systemHint');
  btn.disabled = true;
  try {
    await api('/api/engines/unload', { method: 'POST' });
    await loadEngines();
    await loadSystem();
    hint.className = 'hint ok';
    hint.textContent = t('system.unloaded_all');
    toast(t('system.unloaded_all'));
  } catch (e) {
    hint.className = 'hint err';
    hint.textContent = e.message;
  } finally {
    btn.disabled = false;
  }
};

function syncProxyMode() {
  const mode = $('proxyMode').value;
  $('proxy').hidden = mode !== 'manual';
  const hint = $('proxyHint');
  hint.className = 'hint';
  if (mode === 'system') {
    hint.innerHTML = detectedProxy
      ? t('ui.detected_system_proxy', { proxy: esc(detectedProxy) })
      : t('ui.no_system_proxy');
  } else if (mode === 'manual') {
    hint.textContent = t("ui.used_for_model_downloads_only");
  } else {
    hint.textContent = t("ui.always_connect_directly_even_if");
  }
}
$('proxyMode').onchange = syncProxyMode;

function setFfmpegHint(bin) {
  const hint = $('ffmpegHint');
  hint.className = 'hint';
  hint.textContent = bin
    ? t('settings.ffmpeg.found', { path: bin })
    : t('settings.ffmpeg.missing');
}

async function loadSettingsPage() {
  if (!session.loopback) {
    $('downloadSettings').hidden = true;
    $('phoneSettings').hidden = false;
    return;
  }
  $('downloadSettings').hidden = false;
  $('phoneSettings').hidden = true;
  loadSystem();
  try {
    const s = await api('/api/settings');
    detectedProxy = s.system_proxy || null;
    $('proxyMode').value = s.proxy_mode || 'system';
    $('proxy').value = s.proxy || '';
    syncProxyMode();
    setPreset('hf', s.hf_endpoint || '', HF_PRESETS);
    setPreset('pip', s.pip_index || '', PIP_PRESETS);
    $('workspaceDir').value = s.workspace_dir || '';
    $('ffmpegPath').value = s.ffmpeg_path || '';
    setFfmpegHint(s.ffmpeg_bin || '');
    $('pathModels').textContent = s.models_dir || '—';
    $('pathTools').textContent = s.tools_dir || (t("ui.not_found"));
    $('pathScripts').textContent = s.scripts_dir || (t("ui.not_found"));
    serverSettings = s;
    $('settingsHint').className = 'hint';
    $('settingsHint').textContent = '';
  } catch (e) {
    $('settingsHint').className = 'hint err';
    $('settingsHint').textContent = (t("ui.failed_to_load_settings")) + e.message;
  }
}

$('saveSettings').onclick = async () => {
  const hint = $('settingsHint');
  const body = {
    proxy_mode: $('proxyMode').value,
    proxy: $('proxy').value.trim(),
    hf_endpoint: getPreset('hf'),
    pip_index: getPreset('pip'),
    workspace_dir: $('workspaceDir').value.trim(),
    ffmpeg_path: $('ffmpegPath').value.trim(),
    language: localStorage.getItem('laverock.lang') || '',
  };
  try {
    const saved = await api('/api/settings', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    $('proxyMode').value = saved.proxy_mode || 'system';
    $('proxy').value = saved.proxy || '';
    syncProxyMode();
    setPreset('hf', saved.hf_endpoint || '', HF_PRESETS);
    setPreset('pip', saved.pip_index || '', PIP_PRESETS);
    $('ffmpegPath').value = saved.ffmpeg_path || '';
    setFfmpegHint(saved.ffmpeg_bin || '');
    hint.className = 'hint ok';
    hint.textContent = t("ui.saved_applies_to_the_next");
    toast(t("ui.download_settings_saved"));
    serverSettings = saved;
    // The projects page's file picker follows the detected ffmpeg.
    await loadHealth();
  } catch (e) {
    hint.className = 'hint err';
    hint.textContent = (t("ui.save_failed")) + e.message;
  }
};
