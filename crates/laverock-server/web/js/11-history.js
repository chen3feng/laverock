// ---------------------------------------------------------------- history

let historyItems = [];
let historySearch = '';

// Task chips on the history page. Entries written before `task` existed
// fall back to the output kind.
const HISTORY_TASKS = ['tts', 'music', 'asr', 'vc', 'design', 'process'];
let historyTaskFilter = '';
try {
  historyTaskFilter = localStorage.getItem('laverock-history-task') || '';
} catch (_) {}

function taskOf(h) {
  return h.task || (h.kind === 'music' ? 'music' : 'tts');
}

function taskLabel(task) {
  return t('history.task.' + (task || 'tts'));
}

// Music routes are stored as stable ids; unknown ones (a manifest added
// later) fall back to the raw value instead of an i18n key.
function routeLabel(route) {
  const key = 'history.route.' + route;
  const label = t(key);
  return label === key ? route : label;
}

async function loadHistory() {
  const q = historySearch.trim();
  try {
    const d = await api('/api/history' + (q ? '?q=' + encodeURIComponent(q) : ''));
    historyItems = d.history || [];
  } catch (e) {
    toast((t("ui.failed_to_load_history")) + e.message, true);
    return;
  }
  renderHistory();
}

function renderHistoryFilters() {
  const box = $('historyFilters');
  box.innerHTML = '';
  box.hidden = historyItems.length === 0;
  if (!historyItems.length) return;

  const counts = new Map();
  for (const h of historyItems) {
    const task = taskOf(h);
    counts.set(task, (counts.get(task) || 0) + 1);
  }
  // A filter whose entries were all deleted falls back to "all".
  if (historyTaskFilter && !counts.has(historyTaskFilter)) {
    historyTaskFilter = '';
    try { localStorage.removeItem('laverock-history-task'); } catch (_) {}
  }

  const entries = [{ task: '', label: t('history.filter.all'), count: historyItems.length }];
  for (const task of HISTORY_TASKS) {
    if (!counts.has(task)) continue;
    entries.push({ task, label: taskLabel(task), count: counts.get(task) });
  }
  for (const entry of entries) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'filter-chip' + (historyTaskFilter === entry.task ? ' active' : '');
    btn.setAttribute('aria-pressed', String(historyTaskFilter === entry.task));
    btn.textContent = entry.label;
    const count = document.createElement('span');
    count.className = 'count';
    count.textContent = entry.count;
    btn.appendChild(count);
    btn.onclick = () => {
      historyTaskFilter = entry.task;
      try {
        if (entry.task) localStorage.setItem('laverock-history-task', entry.task);
        else localStorage.removeItem('laverock-history-task');
      } catch (_) {}
      renderHistory();
    };
    box.appendChild(btn);
  }
}

function renderHistory() {
  const box = $('historyList');
  box.innerHTML = '';
  renderHistoryFilters();
  const items = historyTaskFilter
    ? historyItems.filter((h) => taskOf(h) === historyTaskFilter)
    : historyItems;
  const empty = $('historyEmpty');
  empty.textContent = historySearch.trim()
    ? t('history.empty.search')
    : historyTaskFilter && historyItems.length
      ? t('history.empty.task')
      : t('history.empty');
  empty.hidden = items.length > 0;
  $('clearHistory').hidden = historyItems.length === 0;
  const engineName = Object.fromEntries(engines.map((e) => [e.id, e.name]));

  // Entries that share a generation id came from one multi-take request:
  // group them into a single card with per-take rows. Standalone entries
  // render as their own card.
  const groups = [];
  const byGen = new Map();
  for (const h of items) {
    if (h.generation) {
      let g = byGen.get(h.generation);
      if (!g) { g = []; byGen.set(h.generation, g); groups.push(g); }
      g.push(h);
    } else {
      groups.push([h]);
    }
  }
  for (const g of groups) g.sort((a, b) => (a.index || 0) - (b.index || 0));
  groups.sort((a, b) => newest(b) - newest(a)); // newest generation first

  for (const group of groups) {
    const card = document.createElement('div');
    card.className = 'card';
    const multi = group.length > 1;
    if (multi) {
      const head = document.createElement('div');
      head.className = 'hist-meta';
      const chip = document.createElement('span');
      chip.className = 'chip';
      chip.textContent = t('history.variant_group', { n: group.length });
      const when = document.createElement('span');
      when.className = 'hint';
      when.textContent = new Date(newest(group) * 1000)
        .toLocaleString(dateLocale(), { hour12: false });
      head.append(chip, when);
      card.appendChild(head);
    }
    for (let i = 0; i < group.length; i++) {
      if (i > 0) {
        const hr = document.createElement('hr');
        hr.style.cssText = 'border:none;border-top:1px solid var(--line);margin:10px 0';
        card.appendChild(hr);
      }
      buildHistoryTake(card, group[i], engineName,
        multi ? t('history.variant', { n: (group[i].index || 0) + 1 }) : null);
    }
    box.appendChild(card);
  }
}

function newest(group) {
  return Math.max(...group.map((x) => x.created_at));
}

// Jump to the history page already filtered to one task.
function openHistoryTask(task) {
  historyTaskFilter = task;
  try { localStorage.setItem('laverock-history-task', task); } catch (_) {}
  location.hash = '#/history';
}

// A compact "recent results" block for task pages: the newest entries for
// one task, drawn with the same card builder as the history page, plus a
// link to the full list. `onDeleted` refreshes the block after a delete.
async function loadRecentResults(task, hostId, titleId, limit = 5, onDeleted) {
  let items = [];
  try {
    const d = await api('/api/history?task=' + encodeURIComponent(task));
    items = (d.history || []).slice(0, limit);
  } catch (_) {
    return;
  }
  const host = $(hostId);
  host.innerHTML = '';
  $(titleId).hidden = items.length === 0;
  if (!items.length) return;

  const engineName = Object.fromEntries(engines.map((e) => [e.id, e.name]));
  const refresh = onDeleted || (() => loadRecentResults(task, hostId, titleId, limit));
  for (const h of items) {
    const card = document.createElement('div');
    card.className = 'card';
    buildHistoryTake(card, h, engineName, null, refresh);
    host.appendChild(card);
  }

  const more = document.createElement('div');
  more.className = 'actions';
  const link = document.createElement('a');
  link.className = 'hint';
  link.href = '#/history';
  link.textContent = t('recent.view_all');
  link.onclick = (e) => {
    e.preventDefault();
    openHistoryTask(task);
  };
  more.appendChild(link);
  host.appendChild(more);
}

function buildHistoryTake(host, h, engineName, variantLabel, onDeleted) {
  const meta = document.createElement('div');
  meta.className = 'hist-meta';
  const chip = document.createElement('span');
  chip.className = 'chip';
  chip.textContent = engineName[h.engine_id] || h.engine_name || h.engine_id;
  meta.appendChild(chip);
  const task = document.createElement('span');
  task.className = 'chip';
  task.textContent = taskLabel(taskOf(h));
  meta.appendChild(task);
  // Editing routes are the interesting part of a music entry; the default
  // text-to-music route adds nothing beyond the "Music" chip.
  if (h.route && h.route !== 'text2music') {
    const route = document.createElement('span');
    route.className = 'chip';
    route.textContent = routeLabel(h.route);
    meta.appendChild(route);
  }
  // Transcripts name their source clip; audio entries already show it in
  // the text line.
  if (h.output === 'text' && h.input && h.input.name) {
    const source = document.createElement('span');
    source.className = 'chip';
    source.textContent = h.input.name;
    meta.appendChild(source);
  }
  if (variantLabel) {
    const take = document.createElement('span');
    take.className = 'chip';
    take.textContent = variantLabel;
    meta.appendChild(take);
  }
  const when = document.createElement('span');
  when.className = 'hint';
  when.textContent =
    new Date(h.created_at * 1000).toLocaleString(dateLocale(), { hour12: false }) +
    ` · ${fmtDur(h.duration_secs)} · ${fmtBytes(h.bytes)}`;
  meta.appendChild(when);

  // Text results (transcripts) get an editable card instead of a player.
  if (h.output === 'text') {
    host.append(meta, buildHistoryTranscript(h, onDeleted));
    return;
  }

  const text = document.createElement('div');
  text.className = 'hist-text';
  text.textContent = h.text;

  host.append(meta, text, buildHistoryAudioRow(h, onDeleted));
}

// Save a server-side artifact through the same file picker as downloads.
async function historySaveFromUrl(el, url, name, busyLabel) {
  const label = el.textContent;
  el.style.pointerEvents = 'none';
  el.textContent = busyLabel;
  try {
    const res = await apiFetch(url);
    if (!res.ok) throw new Error('HTTP ' + res.status);
    await saveBlob(await res.blob(), name);
  } catch (e) {
    toast((t("ui.download_failed")) + e.message, true);
  } finally {
    el.style.pointerEvents = '';
    el.textContent = label;
  }
}

function historyDeleteButton(h, onDeleted) {
  const del = document.createElement('button');
  del.className = 'ghost danger';
  del.type = 'button';
  del.textContent = t("ui.delete");
  del.onclick = async () => {
    const yes = await confirmAction(
      h.output === 'text'
        ? t('history.delete_text_confirm')
        : t("ui.delete_this_generation_the_audio"),
      { title: t("ui.delete_entry"), okText: t("ui.delete") },
    );
    if (!yes) return;
    del.disabled = true;
    try {
      await api(`/api/history/${encodeURIComponent(h.id)}`, { method: 'DELETE' });
      // Simplest correct refresh: re-fetch so grouped cards stay consistent.
      await (onDeleted ? onDeleted() : loadHistory());
    } catch (e) {
      del.disabled = false;
      toast((t("ui.delete_failed")) + e.message, true);
    }
  };
  return del;
}

function historyPlayer(label, url) {
  const box = document.createElement('div');
  box.className = 'hist-compare';
  if (label) {
    const cap = document.createElement('span');
    cap.className = 'hint';
    cap.textContent = label;
    box.appendChild(cap);
  }
  const audio = document.createElement('audio');
  audio.controls = true;
  audio.preload = 'none';
  audio.addEventListener('play', () => pauseOtherAudio(audio));
  audio.src = url;
  box.appendChild(audio);
  return box;
}

function buildHistoryAudioRow(h, onDeleted) {
  const id = encodeURIComponent(h.id);
  const row = document.createElement('div');
  row.className = 'hist-row';

  // Voice conversion keeps its source clip, so show both sides.
  const hasSource = !!(h.input && h.input.audio);
  if (hasSource) {
    row.append(
      historyPlayer(t('history.source'), `/api/history/${id}/input`),
      historyPlayer(t('history.converted'), `/api/history/${id}/audio`),
    );
  } else {
    const player = historyPlayer('', `/api/history/${id}/audio`);
    player.style.flex = '1';
    row.appendChild(player);
  }

  const dl = document.createElement('a');
  dl.className = 'hint';
  dl.href = `/api/history/${id}/audio`;
  dl.textContent = t('download.wav');
  const wavUrl = dl.href;
  dl.onclick = (e) => {
    e.preventDefault();
    historySaveFromUrl(dl, wavUrl, `laverock-${h.id}.wav`, t("ui.preparing"));
  };
  row.appendChild(dl);

  if (hasSource) {
    const src = document.createElement('a');
    src.className = 'hint';
    src.href = `/api/history/${id}/input`;
    src.textContent = t('history.download_source');
    const srcUrl = src.href;
    src.onclick = (e) => {
      e.preventDefault();
      historySaveFromUrl(src, srcUrl, `laverock-${h.id}-source.wav`, t("ui.preparing"));
    };
    row.appendChild(src);
  }

  if (mp3Supported) {
    const mp3 = document.createElement('a');
    mp3.className = 'hint';
    mp3.href = `/api/history/${id}/audio?format=mp3`;
    mp3.textContent = t('download.mp3');
    const mp3Url = mp3.href;
    mp3.onclick = (e) => {
      e.preventDefault();
      historySaveFromUrl(mp3, mp3Url, `laverock-${h.id}.mp3`, t("ui.converting"));
    };
    row.appendChild(mp3);
  }

  row.append(historyDeleteButton(h, onDeleted));
  return row;
}

function buildHistoryTranscript(h, onDeleted) {
  const box = document.createElement('div');
  const area = document.createElement('textarea');
  area.className = 'hist-transcript';
  area.rows = 5;
  area.spellcheck = false;
  // The list only carries a preview; load the full transcript in place.
  area.value = h.text || '';
  apiFetch(`/api/history/${encodeURIComponent(h.id)}/text`)
    .then((res) => (res.ok ? res.text() : null))
    .then((full) => {
      if (full !== null && !area.dataset.edited) area.value = full;
    })
    .catch(() => {});
  area.addEventListener('input', () => { area.dataset.edited = '1'; });
  box.appendChild(area);

  const row = document.createElement('div');
  row.className = 'hist-row';

  const copy = document.createElement('button');
  copy.type = 'button';
  copy.className = 'ghost small';
  copy.textContent = t('history.copy');
  copy.onclick = async () => {
    let copied = false;
    try {
      await navigator.clipboard.writeText(area.value);
      copied = true;
    } catch (_) {
      area.select();
      copied = document.execCommand('copy');
    }
    toast(copied ? t('history.copied') : t('history.copy_failed'), !copied);
  };

  const txt = document.createElement('a');
  txt.className = 'hint';
  txt.href = '#';
  txt.textContent = t('history.download_txt');
  txt.onclick = async (e) => {
    e.preventDefault();
    await saveBlob(
      new Blob([area.value], { type: 'text/plain;charset=utf-8' }),
      `laverock-${h.id}.txt`,
    );
  };
  row.append(copy, txt);

  if (h.has_segments) {
    for (const format of ['srt', 'vtt']) {
      const link = document.createElement('a');
      link.className = 'hint';
      link.href = `/api/history/${encodeURIComponent(h.id)}/subtitles?format=${format}`;
      link.textContent = t('history.download_' + format);
      const url = link.href;
      link.onclick = (e) => {
        e.preventDefault();
        historySaveFromUrl(link, url, `laverock-${h.id}.${format}`, t("ui.preparing"));
      };
      row.appendChild(link);
    }
  }

  row.append(historyDeleteButton(h, onDeleted));
  box.appendChild(row);
  return box;
}

// Search runs on the server so it can reach full transcripts, not just the
// 2,000-character preview carried in the list.
let historySearchTimer = null;
$('historySearch').addEventListener('input', (e) => {
  clearTimeout(historySearchTimer);
  historySearchTimer = setTimeout(() => {
    historySearch = e.target.value;
    loadHistory();
  }, 200);
});

$('clearHistory').onclick = async () => {
  const yes = await confirmAction(
    t("ui.clear_all_generation_history_this"),
    { title: t("ui.clear_history"), okText: t("ui.clear") },
  );
  if (!yes) return;
  try {
    await api('/api/history', { method: 'DELETE' });
    historyItems = [];
    renderHistory();
    toast(t("ui.history_cleared"));
  } catch (e) {
    toast((t("ui.clear_failed")) + e.message, true);
  }
};
