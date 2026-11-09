// ---------------------------------------------------------------- transcribe

const asrCapture = new Capture($('transcribeCapture'), {
  autoTranscribe: false,
  detailed: true,
  live: true,
});

// The result card shows cues from a fresh run, or a history id when the
// user opens an older transcript (its cues stay on the server).
let transcribeSegments = [];
let transcribeHistoryId = null;
let transcribeHasCues = false;
// True while a streaming request is still delivering cues.
let transcribeStreaming = false;
// Live dictation text shown while recording, and until the pass after
// stop replaces it with the final transcript.
let transcribeLiveText = '';
let transcribeLive = false;

asrCapture.onLiveText = (text) => {
  transcribeLive = true;
  transcribeLiveText = text;
  transcribeStreaming = false;
  transcribeSegments = [];
  transcribeHistoryId = null;
  transcribeHasCues = false;
  showTranscribeResult(text, 'live');
};

asrCapture.onProgress = (segment) => {
  // The live text stays on screen until the final pass finishes.
  if (transcribeLiveText) return;
  transcribeStreaming = true;
  transcribeHistoryId = null;
  transcribeHasCues = true;
  transcribeSegments.push(segment);
  showTranscribeResult(segmentsText(transcribeSegments), 'streaming');
};

asrCapture.onTranscribed = (text, transcript) => {
  transcribeLive = false;
  transcribeLiveText = '';
  transcribeStreaming = false;
  transcribeSegments = (transcript && transcript.segments) || [];
  transcribeHistoryId = null;
  transcribeHasCues = transcribeSegments.length > 0;
  showTranscribeResult(text, 'done');
  loadRecentTranscriptions();
};

// Mirror the server's cue join: CJK needs no space, Latin words do.
function segmentsText(segments) {
  let out = '';
  for (const segment of segments) out = joinCjk(out, segment.text);
  return out;
}

function syncTranscribePage() {
  const hint = $('transcribeHint');
  hint.className = transcribeAvailable ? 'hint' : 'err';
  hint.textContent = transcribeAvailable
    ? t('transcribe.hint')
    : t('transcribe.no_model');
  asrCapture.syncTranscribeButton();
}

function showTranscribeResult(text, state = 'done') {
  $('transcribeResultCard').hidden = false;
  const area = $('transcribeText');
  const atBottom = area.scrollTop + area.clientHeight >= area.scrollHeight - 4;
  area.value = text || '';
  // Follow the growing text unless the user scrolled up to read.
  if (state !== 'done' && atBottom) area.scrollTop = area.scrollHeight;
  const meta = $('transcribeResultMeta');
  meta.innerHTML = '';
  const chip = document.createElement('span');
  chip.className = 'chip';
  chip.textContent = t('transcribe.result');
  meta.appendChild(chip);
  const hint = document.createElement('span');
  hint.className = 'hint';
  hint.textContent = state === 'live'
    ? t('transcribe.live')
    : state === 'streaming' ? t('transcribe.streaming') : t('transcribe.saved');
  meta.appendChild(hint);
  renderTranscribeActions();
}

// Timed cues are rendered here rather than fetched back, so a fresh
// transcript can be exported without another round trip.
function cueStamp(seconds, separator) {
  const ms = Math.max(0, Math.round((Number(seconds) || 0) * 1000));
  const parts = [
    Math.floor(ms / 3_600_000),
    Math.floor((ms % 3_600_000) / 60_000),
    Math.floor((ms % 60_000) / 1000),
  ].map((n) => String(n).padStart(2, '0'));
  return parts.join(':') + separator + String(ms % 1000).padStart(3, '0');
}

function cuesToSrt(segments) {
  return segments
    .map((seg, i) => `${i + 1}\n${cueStamp(seg.start, ',')} --> ${cueStamp(seg.end, ',')}\n${seg.text}\n`)
    .join('\n');
}

function cuesToVtt(segments) {
  return 'WEBVTT\n\n' + segments
    .map((seg) => `${cueStamp(seg.start, '.')} --> ${cueStamp(seg.end, '.')}\n${seg.text}\n`)
    .join('\n');
}

function renderTranscribeActions() {
  const row = $('transcribeActions');
  row.innerHTML = '';
  // Exports only make sense once the transcript is complete.
  if (transcribeLive || transcribeStreaming) return;
  const area = $('transcribeText');

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
      'laverock-transcript.txt',
    );
  };
  row.append(copy, txt);

  const downloadCues = async (format) => {
    const body = format === 'srt' ? cuesToSrt(transcribeSegments) : cuesToVtt(transcribeSegments);
    const mime = format === 'srt'
      ? 'application/x-subrip;charset=utf-8'
      : 'text/vtt;charset=utf-8';
    await saveBlob(new Blob([body], { type: mime }), `laverock-transcript.${format}`);
  };

  for (const format of ['srt', 'vtt']) {
    if (transcribeSegments.length) {
      const link = document.createElement('a');
      link.className = 'hint';
      link.href = '#';
      link.textContent = t('history.download_' + format);
      link.onclick = (e) => {
        e.preventDefault();
        downloadCues(format);
      };
      row.appendChild(link);
    } else if (transcribeHistoryId && transcribeHasCues) {
      const link = document.createElement('a');
      link.className = 'hint';
      link.href = `/api/history/${encodeURIComponent(transcribeHistoryId)}/subtitles?format=${format}`;
      link.textContent = t('history.download_' + format);
      link.onclick = (e) => {
        e.preventDefault();
        historySaveFromUrl(
          link, link.href, `laverock-${transcribeHistoryId}.${format}`, t('ui.preparing'));
      };
      row.appendChild(link);
    }
  }
}

async function openTranscribeEntry(h, button) {
  button.disabled = true;
  try {
    const res = await apiFetch(`/api/history/${encodeURIComponent(h.id)}/text`);
    if (!res.ok) throw new Error('HTTP ' + res.status);
    transcribeLive = false;
    transcribeLiveText = '';
    transcribeStreaming = false;
    transcribeSegments = [];
    transcribeHistoryId = h.id;
    transcribeHasCues = !!h.has_segments;
    showTranscribeResult(await res.text());
    $('transcribeResultCard').scrollIntoView({ behavior: 'smooth', block: 'nearest' });
  } catch (e) {
    toast(t('ui.failed_to_load_history') + e.message, true);
  } finally {
    button.disabled = false;
  }
}

async function loadRecentTranscriptions() {
  let items = [];
  try {
    const d = await api('/api/history?task=asr');
    items = (d.history || []).slice(0, 5);
  } catch (_) {
    return;
  }
  const host = $('transcribeRecent');
  host.innerHTML = '';
  $('transcribeRecentTitle').hidden = items.length === 0;
  if (!items.length) return;

  for (const h of items) {
    const card = document.createElement('div');
    card.className = 'card';

    const meta = document.createElement('div');
    meta.className = 'hist-meta';
    const when = document.createElement('span');
    when.className = 'hint';
    when.textContent = new Date(h.created_at * 1000)
      .toLocaleString(dateLocale(), { hour12: false });
    meta.appendChild(when);
    if (h.input && h.input.name) {
      const source = document.createElement('span');
      source.className = 'chip';
      source.textContent = h.input.name;
      meta.appendChild(source);
    }
    const dur = document.createElement('span');
    dur.className = 'hint';
    dur.textContent = fmtDur(h.duration_secs);
    meta.appendChild(dur);
    card.appendChild(meta);

    const preview = document.createElement('div');
    preview.className = 'hist-text';
    preview.textContent = h.text;
    card.appendChild(preview);

    const row = document.createElement('div');
    row.className = 'hist-row';
    const open = document.createElement('button');
    open.type = 'button';
    open.className = 'ghost small';
    open.textContent = t('transcribe.open');
    open.onclick = () => openTranscribeEntry(h, open);
    row.appendChild(open);
    card.appendChild(row);
    host.appendChild(card);
  }

  const more = document.createElement('div');
  more.className = 'actions';
  const link = document.createElement('a');
  link.className = 'hint';
  link.href = '#/history';
  link.textContent = t('transcribe.open_history');
  link.onclick = (e) => {
    e.preventDefault();
    openHistoryTask('asr');
  };
  more.appendChild(link);
  host.appendChild(more);
}
