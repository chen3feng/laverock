// ---------------------------------------------------------------- subtitle projects

// A project owns one source clip and its timed cues. This stage covers the
// subtitle workflow: import or transcribe, edit the cues against the audio,
// and export SRT/VTT. Per-segment TTS and video packaging come later, but
// the stored schema already carries their fields.
let projectList = [];
let currentProject = null;
let synthJobId = null;

async function loadProjects() {
  try {
    projectList = await api('/api/projects');
  } catch (e) {
    toast(t('projects.load_failed') + e.message, true);
    return;
  }
  renderProjects();
  if (currentProject) {
    updateProjectMeta(currentProject);
    fillProjectPickers(currentProject);
  }
}

function renderProjects() {
  const grid = $('projectGrid');
  if (!grid) return;
  grid.innerHTML = '';
  $('projectsEmpty').hidden = projectList.length > 0;
  for (const project of projectList) {
    const el = document.createElement('div');
    el.className = 'voice-card project-card';
    el.dataset.id = project.id;
    const count = (project.segments || []).length;
    el.innerHTML = `
      <div class="voice-top"><b>${esc(project.name)}</b></div>
      <div class="voice-meta">${fmtDur(project.source.duration)} · ${esc(t('projects.segments', { n: count }))} · ${fmtDate(project.updated_at)}</div>
      <div class="voice-actions">
        <button class="ghost small open" type="button">${esc(t('projects.open'))}</button>
        <button class="ghost small danger del" type="button">${esc(t('ui.delete'))}</button>
      </div>`;
    el.querySelector('.open').onclick = () => openProject(project);
    el.querySelector('.del').onclick = () => deleteProject(project);
    grid.appendChild(el);
  }
}

function updateProjectMeta(project) {
  const count = (project.segments || []).length;
  $('projectMeta').textContent =
    `${fmtDur(project.source.duration)} · ${t('projects.segments', { n: count })}`;
}

function openProject(project) {
  currentProject = project;
  $('projectEditor').hidden = false;
  $('projectTitle').value = project.name;
  updateProjectMeta(project);
  fillProjectPickers(project);
  $('projectSynthStatus').textContent = '';
  $('projectSynthProgress').hidden = true;
  $('projectAudio').src = `/api/projects/${encodeURIComponent(project.id)}/source`;
  renderSegments(project.segments || []);
  $('projectEditor').scrollIntoView({ behavior: 'smooth', block: 'start' });
}

// Speech engines and saved voices, keeping the project's own choice when it
// still exists. Preset engines (Kokoro) work with the empty "engine default"
// voice.
function fillProjectPickers(project) {
  const engineSel = $('projectEngine');
  const speech = engines.filter(
    (e) => (e.task || (e.kind === 'music' ? 'music' : 'tts')) === 'tts');
  engineSel.innerHTML = '';
  if (!speech.length) {
    const o = document.createElement('option');
    o.value = '';
    o.textContent = t('ui.no_models_found_download_one');
    engineSel.appendChild(o);
  }
  for (const e of speech) {
    const o = document.createElement('option');
    o.value = e.id;
    o.textContent = e.name;
    engineSel.appendChild(o);
  }
  if (project.engine_id && speech.some((e) => e.id === project.engine_id)) {
    engineSel.value = project.engine_id;
  }

  const voiceSel = $('projectVoice');
  voiceSel.innerHTML = '';
  const none = document.createElement('option');
  none.value = '';
  none.textContent = t('projects.voice.none');
  voiceSel.appendChild(none);
  for (const p of profiles) {
    const o = document.createElement('option');
    o.value = p.id;
    o.textContent = p.name;
    voiceSel.appendChild(o);
  }
  if (project.voice_id && profiles.some((p) => p.id === project.voice_id)) {
    voiceSel.value = project.voice_id;
  }
}

function closeProjectEditor() {
  currentProject = null;
  $('projectEditor').hidden = true;
  $('projectSegments').innerHTML = '';
  $('projectAudio').pause();
  $('projectAudio').removeAttribute('src');
  $('projectSegAudio').pause();
  $('projectSegAudio').removeAttribute('src');
  $('projectSynthStatus').textContent = '';
  $('projectSynthProgress').hidden = true;
}

function renderSegments(segments) {
  const host = $('projectSegments');
  host.innerHTML = '';
  segments.forEach((segment, index) => host.appendChild(segmentRow(segment, index)));
}

function segmentRow(segment, index) {
  const row = document.createElement('div');
  row.className = 'seg-row';
  row.dataset.id = segment.id || '';
  const start = Number.isFinite(segment.start) ? segment.start : 0;
  const end = Number.isFinite(segment.end) ? segment.end : start + 1;
  const hasAudio = !!segment.tts_path;
  row.innerHTML = `
    <span class="seg-index">${index + 1}</span>
    <input class="seg-start" type="number" min="0" step="0.01" value="${start.toFixed(2)}" />
    <span class="seg-arrow">→</span>
    <input class="seg-end" type="number" min="0" step="0.01" value="${end.toFixed(2)}" />
    <div class="seg-texts">
      <textarea class="seg-text" rows="2">${esc(segment.text || '')}</textarea>
      <textarea class="seg-translated" rows="1" data-i18n-ph="projects.translation.ph"></textarea>
    </div>
    <div class="seg-actions">
      <button class="ghost small seg-play" type="button" title="${esc(t('projects.play'))}"${hasAudio ? '' : ' hidden'}>▶</button>
      <button class="ghost small seg-rerun" type="button" title="${esc(t('projects.rerun'))}">↻</button>
      <button class="ghost small danger seg-del" type="button" title="${esc(t('ui.delete'))}">×</button>
    </div>`;
  row.querySelector('.seg-translated').value = segment.translated || '';
  applyI18n(row);
  row.querySelector('.seg-play').onclick = () => playSegment(segment.id);
  const rerun = row.querySelector('.seg-rerun');
  if (segment.id) rerun.onclick = () => synthesizeProject([segment.id]);
  else rerun.disabled = true;
  row.querySelector('.seg-del').onclick = () => {
    row.remove();
    renumberSegments();
  };
  return row;
}

function renumberSegments() {
  document.querySelectorAll('#projectSegments .seg-index').forEach((el, index) => {
    el.textContent = String(index + 1);
  });
}

// Keep the fields this stage does not edit (translation, TTS, status) from
// being reset by a subtitle-only save.
function collectSegments() {
  const existing = new Map((currentProject?.segments || []).map((s) => [s.id, s]));
  return [...document.querySelectorAll('#projectSegments .seg-row')].map((row, index) => {
    const id = row.dataset.id || `s${index + 1}`;
    const old = existing.get(id) || {};
    return {
      ...old,
      id,
      start: parseFloat(row.querySelector('.seg-start').value) || 0,
      end: parseFloat(row.querySelector('.seg-end').value) || 0,
      text: row.querySelector('.seg-text').value,
      translated: row.querySelector('.seg-translated').value,
    };
  });
}

async function saveProject() {
  if (!currentProject) return;
  try {
    currentProject = await api(`/api/projects/${encodeURIComponent(currentProject.id)}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        name: $('projectTitle').value,
        engine_id: $('projectEngine').value,
        voice_id: $('projectVoice').value,
        segments: collectSegments(),
      }),
    });
    toast(t('projects.saved'));
    await loadProjects();
    openProject(currentProject);
  } catch (e) {
    toast(t('ui.failed') + e.message, true);
  }
}

async function transcribeProject() {
  if (!currentProject) return;
  if (!transcribeAvailable) {
    toast(t('ui.install_the_sensevoice_speech_to'), true);
    return;
  }
  const button = $('projectTranscribe');
  button.disabled = true;
  toast(t('projects.transcribing'));
  try {
    currentProject = await api(
      `/api/projects/${encodeURIComponent(currentProject.id)}/transcribe`,
      { method: 'POST' },
    );
    toast(t('projects.transcribed'));
    await loadProjects();
    openProject(currentProject);
  } catch (e) {
    toast(t('ui.transcription_failed') + e.message, true);
  } finally {
    button.disabled = false;
  }
}

async function synthesizeProject(only = []) {
  if (!currentProject || synthJobId) return;
  // Persist the engine/voice choice (and any cue edits) first: the worker
  // reads the saved project, and an edited cue's old audio is invalidated.
  try {
    currentProject = await api(`/api/projects/${encodeURIComponent(currentProject.id)}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        engine_id: $('projectEngine').value,
        voice_id: $('projectVoice').value,
        segments: collectSegments(),
      }),
    });
  } catch (e) {
    toast(t('ui.failed') + e.message, true);
    return;
  }
  const fd = new FormData();
  for (const id of only) fd.append('segment', id);
  const progress = $('projectSynthProgress');
  const status = $('projectSynthStatus');
  progress.hidden = false;
  progress.value = 0;
  status.textContent = t('projects.synth.starting');
  $('projectSynthesize').disabled = true;
  try {
    const result = await runJob(
      `/api/projects/${encodeURIComponent(currentProject.id)}/synthesize`,
      fd,
      {
        started: ({ id }) => {
          synthJobId = id;
          $('projectSynthCancel').hidden = false;
        },
        stage: (e) => { status.textContent = e.message; },
        progress: (e) => { progress.value = e.value; },
      },
    );
    status.textContent = result.cancelled
      ? t('projects.synth.cancelled')
      : t('projects.synth.done');
    currentProject = await api(`/api/projects/${encodeURIComponent(currentProject.id)}`);
    renderSegments(currentProject.segments || []);
    updateProjectMeta(currentProject);
    await loadProjects();
  } catch (e) {
    status.textContent = t('projects.synth.failed') + e.message;
  } finally {
    synthJobId = null;
    $('projectSynthesize').disabled = false;
    $('projectSynthCancel').hidden = true;
  }
}

async function cancelSynthesize() {
  if (!synthJobId) return;
  $('projectSynthCancel').disabled = true;
  try {
    await cancelJob(synthJobId);
  } catch (e) {
    toast(t('ui.failed') + e.message, true);
  } finally {
    $('projectSynthCancel').disabled = false;
  }
}

function playSegment(segmentId) {
  if (!currentProject || !segmentId) return;
  const player = $('projectSegAudio');
  const url = `/api/projects/${encodeURIComponent(currentProject.id)}/segments/${encodeURIComponent(segmentId)}/audio`;
  if (player.dataset.segment === segmentId && !player.paused) {
    player.pause();
    return;
  }
  player.dataset.segment = segmentId;
  player.src = url;
  player.play().catch(() => toast(t('ui.failed'), true));
}

async function deleteProject(project) {
  if (!(await confirmAction(t('projects.delete_confirm').replace('{name}', project.name)))) return;
  try {
    await api(`/api/projects/${encodeURIComponent(project.id)}`, { method: 'DELETE' });
    toast(t('projects.deleted'));
    if (currentProject && currentProject.id === project.id) closeProjectEditor();
    await loadProjects();
  } catch (e) {
    toast(t('ui.failed') + e.message, true);
  }
}

async function downloadProjectSubtitles(format) {
  if (!currentProject) return;
  try {
    const res = await apiFetch(
      `/api/projects/${encodeURIComponent(currentProject.id)}/subtitles?format=${format}`);
    if (!res.ok) {
      const body = await res.json().catch(() => ({}));
      throw new Error(body.error || res.statusText || ('HTTP ' + res.status));
    }
    const name = (currentProject.name || 'laverock').replace(/[\\/:*?"<>|]+/g, '_');
    await saveBlob(await res.blob(), `${name}.${format}`);
  } catch (e) {
    toast(t('ui.download_failed') + e.message, true);
  }
}

function addSegment() {
  const rows = document.querySelectorAll('#projectSegments .seg-row');
  const last = rows.length
    ? parseFloat(rows[rows.length - 1].querySelector('.seg-end').value) || 0
    : 0;
  const row = segmentRow({ start: last, end: last + 2, text: '' }, rows.length);
  $('projectSegments').appendChild(row);
}

const projectCapture = new Capture($('projectCapture'));
// The project has its own transcription step; keep the shared component's
// per-clip transcribe button out of this form.
projectCapture.syncTranscribeButton = () => { projectCapture.transBtn.hidden = true; };
projectCapture.syncTranscribeButton();
if (!window.isSecureContext) {
  projectCapture.rec.disabled = true;
  projectCapture.rec.title = t('ui.microphone_access_requires_https_or');
}

async function createProject() {
  if (!projectCapture.blob) {
    toast(t('projects.need_audio'), true);
    return;
  }
  if (projectCapture.blob.size > 200 * 1024 * 1024) {
    toast(t('projects.too_large'), true);
    return;
  }
  const fd = new FormData();
  fd.append('audio', projectCapture.blob, projectCapture.filename || 'source.wav');
  fd.append('name', $('projectName').value.trim() || t('projects.default_name'));
  const subtitleFile = $('projectSubtitles').files[0];
  if (subtitleFile) fd.append('subtitles', await subtitleFile.text());

  const button = $('projectCreate');
  button.disabled = true;
  $('projectCreateHint').textContent = t('projects.creating');
  try {
    let project = await api('/api/projects', { method: 'POST', body: fd });
    // Without an imported subtitle file, transcribe immediately so the
    // editor opens with cues. A failure leaves the project empty and the
    // editor's Re-transcribe button can retry.
    if (!project.segments.length && transcribeAvailable) {
      try {
        project = await api(
          `/api/projects/${encodeURIComponent(project.id)}/transcribe`,
          { method: 'POST' },
        );
      } catch (e) {
        toast(t('projects.transcribe_failed') + e.message, true);
      }
    }
    $('newProjectCard').hidden = true;
    $('projectName').value = '';
    $('projectSubtitles').value = '';
    projectCapture.clear();
    await loadProjects();
    openProject(project);
  } catch (e) {
    toast(t('ui.failed') + e.message, true);
  } finally {
    button.disabled = false;
    $('projectCreateHint').textContent = '';
  }
}

$('newProjectBtn').onclick = () => {
  $('newProjectCard').hidden = false;
  $('projectCreateHint').textContent = '';
};
$('projectCreateCancel').onclick = () => {
  $('newProjectCard').hidden = true;
  projectCapture.clear();
  $('projectName').value = '';
  $('projectSubtitles').value = '';
};
$('projectCreate').onclick = createProject;
$('projectSave').onclick = saveProject;
$('projectTranscribe').onclick = transcribeProject;
$('projectSrt').onclick = () => downloadProjectSubtitles('srt');
$('projectVtt').onclick = () => downloadProjectSubtitles('vtt');
$('projectDelete').onclick = () => {
  if (currentProject) deleteProject(currentProject);
};
$('projectClose').onclick = closeProjectEditor;
$('projectAddSegment').onclick = addSegment;
$('projectSynthesize').onclick = () => synthesizeProject();
$('projectSynthCancel').onclick = cancelSynthesize;
