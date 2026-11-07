// ---------------------------------------------------------------- music

function wireMusicSamplesStepper() {
  const input = $('musicSamples');
  const clamp = () => {
    let n = Math.round(Number(input.value));
    if (!Number.isFinite(n)) n = 1;
    input.value = Math.max(1, Math.min(2, n));
  };
  input.addEventListener('change', clamp);
  $('musicSamplesMinus').onclick = () => {
    input.value = Math.max(1, Number(input.value) - 1);
  };
  $('musicSamplesPlus').onclick = () => {
    input.value = Math.min(2, Number(input.value) + 1);
  };
}
wireMusicSamplesStepper();

// Extra buttons for the music page's parameter grid.
function musicParamActions() {
  return {
    lyrics: {
      label: t('music.estimate_duration'),
      title: t('music.estimate_duration.hint'),
      onClick: estimateLyricsDuration,
    },
  };
}

// Estimate a fitting duration from the lyrics: about 3.5 Chinese
// characters or 2.7 English words per second, plus room for an
// intro/outro and one break per instrumental tag. The slider stays a
// target, so the planner can still stop earlier.
function estimateLyricsDuration() {
  const box = $('music-param-lyrics');
  const slider = $('music-param-duration_sec');
  if (!box || !slider) return;

  const raw = box.value || '';
  const instrumental =
    (raw.match(/\[(instrumental|interlude|solo|break|outro)\]/gi) || []).length;
  const text = raw.replace(/\[[^\]]*\]/g, ' ');
  const zh = (text.match(/[㐀-鿿豈-﫿]/g) || []).length;
  const words = (text
    .replace(/[㐀-鿿豈-﫿]/g, ' ')
    .match(/[A-Za-z0-9]+(?:['’-][A-Za-z0-9]+)*/g) || []).length;
  if (zh + words === 0) {
    toast(t('music.lyrics_empty'), true);
    return;
  }

  const seconds = Math.max(10, Math.min(600,
    Math.round((zh / 3.5 + words / 2.7 + 20 + instrumental * 10) / 5) * 5));
  slider.value = String(seconds);
  slider.dispatchEvent(new Event('input', { bubbles: true }));
  toast(t('music.duration_estimated', { seconds }));
}

$('musicGo').onclick = async () => {
  const status = $('musicStatus');
  const caption = $('musicCaption').value.trim();
  const engine = engines.find((x) => x.id === $('musicEngine').value);
  if (!engine) {
    status.className = 'err';
    status.textContent = t('music.no_engine');
    return;
  }
  if (!caption) {
    status.className = 'err';
    status.textContent = t('music.caption_required');
    return;
  }

  const params = collectParams('musicParams');
  const fd = new FormData();
  fd.append('text', caption);
  for (const [name, value] of Object.entries(params)) fd.append(name, value);
  fd.append('samples', $('musicSamples').value);

  const url = `/api/engines/${encodeURIComponent(engine.id)}/synthesize`;
  $('musicGo').disabled = true;
  status.className = '';

  const labelEl = $('musicProgressLabel');
  const pbar = $('musicProgress');
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
  fill.classList.add('indeterminate');
  fill.style.width = '';
  setStage(engine.loaded ? t('music.generating') : t('music.first_load'));
  const timer = setInterval(() => {
    labelEl.querySelector('b').textContent = fmtElapsed();
  }, 250);
  const totalSteps = Number(params.num_steps) || 0;

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

    const reader = res.body.getReader();
    const decoder = new TextDecoder();
    let buf = '';
    const variants = [];
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
          setStage(data + '…');
        } else if (event === 'steps' && totalSteps > 0) {
          const steps = Number(data);
          if (Number.isFinite(steps)) setDeterminate((steps / totalSteps) * 100);
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
    renderSynthResults(got, engine, 'musicResults');
    lastSynthBlob = got[got.length - 1].blob;
    if (engine) engine.loaded = true;
    status.className = 'ok';
    const totalBytes = got.reduce((s, v) => s + v.blob.size, 0);
    status.textContent =
      `${t("ui.done")} · ${got.length > 1 ? got.length + '× · ' : ''}` +
      `${(totalBytes / 1024).toFixed(0)} KB`;
  } catch (e) {
    status.className = 'err';
    status.textContent = (t("ui.failed")) + e.message;
  } finally {
    clearInterval(timer);
    pbar.hidden = true;
    labelEl.hidden = true;
    $('musicGo').disabled = false;
  }
};
