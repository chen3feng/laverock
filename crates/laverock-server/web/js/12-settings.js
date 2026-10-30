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

async function loadSettingsPage() {
  if (!session.loopback) {
    $('downloadSettings').hidden = true;
    $('phoneSettings').hidden = false;
    return;
  }
  $('downloadSettings').hidden = false;
  $('phoneSettings').hidden = true;
  try {
    const s = await api('/api/settings');
    detectedProxy = s.system_proxy || null;
    $('proxyMode').value = s.proxy_mode || 'system';
    $('proxy').value = s.proxy || '';
    syncProxyMode();
    setPreset('hf', s.hf_endpoint || '', HF_PRESETS);
    setPreset('pip', s.pip_index || '', PIP_PRESETS);
    $('workspaceDir').value = s.workspace_dir || '';
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
    hint.className = 'hint ok';
    hint.textContent = t("ui.saved_applies_to_the_next");
    toast(t("ui.download_settings_saved"));
    serverSettings = saved;
  } catch (e) {
    hint.className = 'hint err';
    hint.textContent = (t("ui.save_failed")) + e.message;
  }
};
