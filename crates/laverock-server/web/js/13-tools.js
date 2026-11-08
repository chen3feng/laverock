// ---------------------------------------------------------------- audio tools

// Standalone denoise / separation page. It reuses the capture component for
// upload and recording, runs the work as a background job (so a long
// separation survives a refresh and can be cancelled), and keeps the result
// in the job directory rather than adding it to the library.
let toolsStatus = { denoise: false, separation: false, mp3: false };
let toolJobId = null;
let toolArtifacts = [];
let toolBeforeUrl = null;

// Denoising needs only the small denoiser; every other mode runs the
// separator (which produces both stems).
const toolModeAvailable = (mode) =>
  mode === 'denoise' ? toolsStatus.denoise : toolsStatus.separation;

const toolModeEnabled = () =>
  [...$('toolMode').options].some((opt) => !opt.disabled);

async function loadToolsPage() {
  try {
    toolsStatus = await api('/api/tools');
  } catch (_) {
    toolsStatus = { denoise: false, separation: false, mp3: false };
  }
  const select = $('toolMode');
  for (const opt of select.options) opt.disabled = !toolModeAvailable(opt.value);
  if (!toolModeAvailable(select.value)) {
    const first = [...select.options].find((opt) => !opt.disabled);
    if (first) select.value = first.value;
  }
  const missing = [];
  if (!toolsStatus.denoise) missing.push(t('tools.missing.denoise'));
  if (!toolsStatus.separation) missing.push(t('tools.missing.separation'));
  $('toolModelsHint').innerHTML = missing.length
    ? `${esc(missing.join(' '))} <a href="#/models">${esc(t('tools.missing.link'))}</a>`
    : '';
  $('toolRun').disabled = !toolModeEnabled();
}

function addToolArtifact(view) {
  if (!view || !view.name) return;
  if (!toolArtifacts.some((a) => a.name === view.name)) toolArtifacts.push(view);
}

function toolArtifactLabel(name) {
  if (name.includes('vocals')) return t('tools.track.vocals');
  if (name.includes('instrumental')) return t('tools.track.instrumental');
  return t('tools.track.denoised');
}

function toolDownloadButton(artifact, label, mp3 = false) {
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'ghost small';
  btn.textContent = label;
  btn.onclick = () => downloadToolArtifact(artifact, mp3);
  return btn;
}

async function downloadToolArtifact(artifact, mp3) {
  try {
    const res = await apiFetch(artifact.url);
    if (!res.ok) throw new Error(res.statusText || ('HTTP ' + res.status));
    let blob = await res.blob();
    let name = 'laverock-' + artifact.name;
    if (mp3) {
      const encoded = await apiFetch('/api/audio/mp3', {
        method: 'POST',
        headers: { 'Content-Type': 'audio/wav' },
        body: blob,
      });
      if (!encoded.ok) {
        const body = await encoded.json().catch(() => ({}));
        throw new Error(body.error || encoded.statusText || ('HTTP ' + encoded.status));
      }
      blob = await encoded.blob();
      name = name.replace(/\.wav$/i, '.mp3');
    }
    await saveBlob(blob, name);
  } catch (e) {
    toast(t('ui.download_failed') + e.message, true);
  }
}

// Show the uploaded clip next to each produced stem. `both` yields two WAVs
// plus a ZIP; the players read the WAVs, the ZIP is just a bundle.
function renderToolResult() {
  if (!toolArtifacts.length) return;
  $('toolResult').hidden = false;

  const after = $('toolAfterPlayers');
  after.innerHTML = '';
  const wavs = toolArtifacts.filter((a) => /\.wav$/i.test(a.name));
  $('toolAfterTrack').hidden = wavs.length === 0;
  for (const artifact of wavs) {
    const wrap = document.createElement('div');
    wrap.className = 'tool-player';
    const label = document.createElement('b');
    label.textContent = toolArtifactLabel(artifact.name);
    const player = document.createElement('audio');
    player.controls = true;
    player.src = artifact.url;
    wrap.append(label, player);
    after.appendChild(wrap);
  }

  const downloads = $('toolDownloads');
  downloads.innerHTML = '';
  for (const artifact of toolArtifacts) {
    const zip = /\.zip$/i.test(artifact.name);
    downloads.appendChild(toolDownloadButton(artifact, zip ? 'ZIP' : 'WAV'));
    if (!zip && toolsStatus.mp3) {
      downloads.appendChild(toolDownloadButton(artifact, 'MP3', true));
    }
  }
}

async function runTools() {
  if (!toolsCapture.blob) {
    toast(t('tools.need_audio'), true);
    return;
  }
  const mode = $('toolMode').value;
  if (!toolModeAvailable(mode)) {
    toast(t('tools.missing.denoise'), true);
    return;
  }
  const fd = new FormData();
  fd.append('audio', toolsCapture.blob, toolsCapture.filename || 'input.wav');
  fd.append('mode', mode);

  toolArtifacts = [];
  toolJobId = null;
  $('toolResult').hidden = true;
  $('toolDownloads').innerHTML = '';
  $('toolStatus').textContent = t('tools.starting');
  $('toolProgress').hidden = false;
  $('toolProgress').value = 0;
  $('toolRun').disabled = true;
  $('toolCancel').hidden = false;

  try {
    const result = await runJob('/api/tools/enhance', fd, {
      started: (e) => { toolJobId = e.id; },
      snapshot: (s) => {
        $('toolProgress').value = s.progress || 0;
        for (const artifact of s.artifacts || []) addToolArtifact(artifact);
      },
      stage: (e) => { $('toolStatus').textContent = e.message; },
      progress: (e) => { $('toolProgress').value = e.value; },
      artifact: (e) => addToolArtifact(e),
    });
    if (result && result.cancelled) {
      $('toolStatus').textContent = t('tools.cancelled');
      return;
    }
    $('toolStatus').textContent = t('tools.done');
    renderToolResult();
  } catch (e) {
    $('toolStatus').textContent = t('tools.failed') + e.message;
    toast(t('tools.failed') + e.message, true);
  } finally {
    $('toolProgress').hidden = true;
    $('toolCancel').hidden = true;
    toolJobId = null;
    $('toolRun').disabled = !toolModeEnabled();
  }
}

async function cancelTools() {
  if (!toolJobId) return;
  try {
    await cancelJob(toolJobId);
    $('toolStatus').textContent = t('tools.cancelling');
  } catch (e) {
    toast(t('ui.failed') + e.message, true);
  }
}

const toolsCapture = new Capture($('toolsCapture'));
// The tools page has no use for a transcript; keep that button out of the
// shared component.
toolsCapture.syncTranscribeButton = () => { toolsCapture.transBtn.hidden = true; };
toolsCapture.syncTranscribeButton();
toolsCapture.onChange = () => {
  if (toolBeforeUrl) URL.revokeObjectURL(toolBeforeUrl);
  toolBeforeUrl = toolsCapture.blob ? URL.createObjectURL(toolsCapture.blob) : null;
  const before = $('toolBefore');
  if (toolBeforeUrl) before.src = toolBeforeUrl;
  else before.removeAttribute('src');
  before.hidden = !toolBeforeUrl;
  // A different clip invalidates the previous result.
  $('toolResult').hidden = true;
  toolArtifacts = [];
};
if (!window.isSecureContext) {
  toolsCapture.rec.disabled = true;
  toolsCapture.rec.title = t('ui.microphone_access_requires_https_or');
}

$('toolRun').onclick = runTools;
$('toolCancel').onclick = cancelTools;
