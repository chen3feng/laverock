// ---------------------------------------------------------------- history

let historyItems = [];

async function loadHistory() {
  try {
    const d = await api('/api/history');
    historyItems = d.history || [];
  } catch (e) {
    toast((t("ui.failed_to_load_history")) + e.message, true);
    return;
  }
  renderHistory();
}

function renderHistory() {
  const box = $('historyList');
  box.innerHTML = '';
  $('historyEmpty').hidden = historyItems.length > 0;
  $('clearHistory').hidden = historyItems.length === 0;
  const engineName = Object.fromEntries(engines.map((e) => [e.id, e.name]));

  // Entries that share a generation id came from one multi-take request:
  // group them into a single card with per-take rows. Standalone entries
  // render as their own card.
  const groups = [];
  const byGen = new Map();
  for (const h of historyItems) {
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

function buildHistoryTake(host, h, engineName, variantLabel) {
  const meta = document.createElement('div');
  meta.className = 'hist-meta';
  const chip = document.createElement('span');
  chip.className = 'chip';
  chip.textContent = engineName[h.engine_id] || h.engine_name || h.engine_id;
  meta.appendChild(chip);
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

  const text = document.createElement('div');
  text.className = 'hist-text';
  text.textContent = h.text;

  const row = document.createElement('div');
  row.className = 'hist-row';
  const audio = document.createElement('audio');
  audio.controls = true;
  audio.preload = 'none';
  audio.addEventListener('play', () => pauseOtherAudio(audio));
  audio.src = `/api/history/${encodeURIComponent(h.id)}/audio`;

  const saveFromUrl = async (el, url, name, busyLabel) => {
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
  };

  const dl = document.createElement('a');
  dl.className = 'hint';
  dl.href = audio.src;
  dl.textContent = t('download.wav');
  dl.onclick = (e) => {
    e.preventDefault();
    saveFromUrl(dl, audio.src, `laverock-${h.id}.wav`,
      t("ui.preparing"));
  };

  if (mp3Supported) {
    const mp3 = document.createElement('a');
    mp3.className = 'hint';
    mp3.href = `/api/history/${encodeURIComponent(h.id)}/audio?format=mp3`;
    mp3.textContent = t('download.mp3');
    const mp3Url = mp3.href;
    mp3.onclick = (e) => {
      e.preventDefault();
      saveFromUrl(mp3, mp3Url, `laverock-${h.id}.mp3`,
        t("ui.converting"));
    };
    row.append(audio, dl, mp3);
  } else {
    row.append(audio, dl);
  }

  const del = document.createElement('button');
  del.className = 'ghost danger';
  del.type = 'button';
  del.textContent = t("ui.delete");
  del.onclick = async () => {
    const yes = await confirmAction(
      t("ui.delete_this_generation_the_audio"),
      { title: t("ui.delete_entry"), okText: t("ui.delete") },
    );
    if (!yes) return;
    del.disabled = true;
    try {
      await api(`/api/history/${encodeURIComponent(h.id)}`, { method: 'DELETE' });
      // Simplest correct refresh: re-fetch so grouped cards stay consistent.
      await loadHistory();
    } catch (e) {
      del.disabled = false;
      toast((t("ui.delete_failed")) + e.message, true);
    }
  };
  row.append(del);

  host.append(meta, text, row);
}

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
