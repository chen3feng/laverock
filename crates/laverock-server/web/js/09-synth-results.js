// --------------------------------------------------------- synth results

// Object URLs handed out by renderSynthResults, revoked on the next render
// of the same host (the speech and music pages each keep their own list).
const synthResultUrls = new Map();

function mkDownloadLink(label, handler) {
  const a = document.createElement('a');
  a.className = 'hint';
  a.href = '#';
  a.textContent = label;
  a.onclick = (e) => { e.preventDefault(); handler(a); };
  return a;
}

function renderSynthResults(list, engine, hostId = 'synthResults') {
  const box = $(hostId);
  for (const url of synthResultUrls.get(hostId) || []) URL.revokeObjectURL(url);
  synthResultUrls.set(hostId, []);
  box.innerHTML = '';
  box.hidden = false;
  const multi = list.length > 1;
  for (const v of list) {
    const card = document.createElement('div');
    card.className = 'card';
    card.style.marginTop = '12px';
    if (multi) {
      const chip = document.createElement('span');
      chip.className = 'chip';
      chip.textContent = t('history.variant', { n: v.index + 1 });
      card.appendChild(chip);
    }
    const audio = document.createElement('audio');
    audio.controls = true;
    audio.preload = 'none';
    audio.addEventListener('play', () => pauseOtherAudio(audio));
    const url = URL.createObjectURL(v.blob);
    synthResultUrls.get(hostId).push(url);
    audio.src = url;
    card.appendChild(audio);

    const row = document.createElement('div');
    row.className = 'hist-row';
    row.appendChild(mkDownloadLink(t('download.wav'), () =>
      saveBlob(v.blob, audioFileName('wav', engine && engine.id))));
    if (mp3Supported) {
      row.appendChild(mkDownloadLink(t('download.mp3'), async (link) => {
        const label = link.textContent;
        link.style.pointerEvents = 'none';
        link.textContent = t("ui.converting");
        try {
          const res = await apiFetch('/api/audio/mp3', { method: 'POST', body: v.blob });
          if (!res.ok) {
            const msg = await res.json().then((j) => j.error).catch(() => res.statusText);
            throw new Error(msg);
          }
          await saveBlob(await res.blob(), audioFileName('mp3', engine && engine.id));
        } catch (e) {
          toast((t("ui.mp3_export_failed")) + e.message, true);
        } finally {
          link.style.pointerEvents = '';
          link.textContent = label;
        }
      }));
    }
    card.appendChild(row);
    box.appendChild(card);
  }
}
