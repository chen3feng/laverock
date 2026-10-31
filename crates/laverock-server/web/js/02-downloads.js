// ---------------------------------------------------------------- downloads

// Save a blob while letting the user choose the directory and file name:
// 1. File System Access API (Chrome/Edge/WebView2): a native Save As
//    dialog with an editable name;
// 2. anchor download: desktop WKWebView forwards it to the native
//    download hook (which shows the same kind of panel), plain browsers
//    use their download directory with the suggested name.
async function saveBlob(blob, suggestedName) {
  if (window.showSaveFilePicker) {
    try {
      const ext = (suggestedName.match(/\.([a-z0-9]+)$/i) || [, ''])[1].toLowerCase();
      const types = ext
        ? [{ description: ext.toUpperCase() + ' audio', accept: { [`audio/${ext === 'mp3' ? 'mpeg' : ext}`]: ['.' + ext] } }]
        : [];
      const handle = await window.showSaveFilePicker({ suggestedName, types });
      const writable = await handle.createWritable();
      await writable.write(blob);
      await writable.close();
      return;
    } catch (e) {
      if (e && e.name === 'AbortError') return; // user cancelled the panel
      // fall through to the anchor path on any other failure
    }
  }
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = suggestedName;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10000);
}

function audioFileName(ext, engineId) {
  const d = new Date();
  const p = (n) => String(n).padStart(2, '0');
  const stamp = `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
  let engine = (engineId || '').replace(/^sherpa-onnx-/, '');
  if (engine.length > 24) {
    engine = engine.slice(0, 25).split('-').slice(0, -1).join('-');
  }
  return `laverock-${stamp}${engine ? '-' + engine : ''}.${ext}`;
}

const esc = (value) => String(value ?? '').replace(/[&<>"']/g, (c) => (
  { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]
));
const fmtBytes = (b) => (!b ? '—' : b >= 1048576 ? (b / 1048576).toFixed(1) + ' MB' : Math.max(1, Math.round(b / 1024)) + ' KB');
const fmtDur = (s) => (s ? s.toFixed(1) + 's' : '—');
const fmtDate = (t) => (t ? new Date(t * 1000).toLocaleDateString(dateLocale()) : '—');

function toast(message, isError) {
  const el = $('toast');
  el.textContent = message;
  el.className = 'toast' + (isError ? ' err' : '');
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { el.hidden = true; }, 3200);
}

// Only one clip plays at a time. Voice and take previews share the hidden
// `#previewer`, but the result and history cards use native players and
// the comparison row creates its own, so each playback path stops the
// others explicitly.
function pauseOtherAudio(except) {
  const previewer = $('previewer');
  const stoppedPreview = previewer !== except && !previewer.paused;
  for (const el of document.querySelectorAll('audio')) {
    if (el !== except) el.pause();
  }
  if (stoppedPreview) resetPreviewState();
}

// In-app modal. WebKit's `confirm()` is not implemented inside the Tauri
// WKWebView (it returns false without showing anything), so every
// confirmation and form goes through this dialog instead.
let dialogResolve = null;

function appendHint(wrap, field) {
  if (!field.hint) return;
  const hint = document.createElement('div');
  hint.className = 'hint';
  hint.textContent = field.hint;
  wrap.appendChild(hint);
}

function showDialog({ title, message = '', fields = [], okText, danger = false }) {
  okText = okText ?? t('dialog.ok');
  return new Promise((resolve) => {
    dialogResolve = resolve;
    $('dialogTitle').textContent = title;
    const msg = $('dialogMessage');
    msg.textContent = message;
    msg.hidden = !message;

    const host = $('dialogFields');
    host.innerHTML = '';
    for (const f of fields) {
      const wrap = document.createElement('div');
      wrap.style.marginTop = '12px';
      const label = document.createElement('label');
      label.textContent = f.label;
      label.htmlFor = 'dialog-f-' + f.name;
      wrap.appendChild(label);
      let input;
      if (f.type === 'textarea') {
        input = document.createElement('textarea');
        input.rows = 3;
        input.value = f.value || '';
      } else if (f.type === 'select') {
        input = document.createElement('select');
        for (const o of f.options || []) {
          const opt = document.createElement('option');
          opt.value = o.value;
          opt.textContent = o.label;
          if (o.value === f.value) opt.selected = true;
          input.appendChild(opt);
        }
      } else if (f.type === 'file') {
        input = document.createElement('input');
        input.type = 'file';
        input.accept = f.accept || 'audio/wav,.wav';
        input.style.color = 'var(--muted)';
      } else if (f.type === 'audio') {
        // A row of play buttons for comparing two renditions of the same
        // clip. It carries no value: the buttons are not form fields, so
        // only the id (for the label) is set below.
        input = document.createElement('div');
        input.className = 'compare-audio';
        const player = new Audio();
        player.preload = 'none';
        input.appendChild(player);
        for (const o of f.options || []) {
          const btn = document.createElement('button');
          btn.type = 'button';
          btn.className = 'ghost';
          btn.textContent = o.label;
          btn.onclick = () => {
            if (player.dataset.url === o.url) {
              if (player.paused) {
                pauseOtherAudio(player);
                player.play().catch(() => {});
              } else {
                player.pause();
              }
              return;
            }
            pauseOtherAudio(player);
            player.pause();
            player.dataset.url = o.url;
            player.src = o.url;
            player.play().catch(() => {});
          };
          input.appendChild(btn);
        }
      } else {
        input = document.createElement('input');
        input.type = 'text';
        input.value = f.value || '';
        if (f.placeholder) input.placeholder = f.placeholder;
      }
      input.id = 'dialog-f-' + f.name;
      if (f.type !== 'audio') {
        input.dataset.field = f.name;
        input.dataset.fieldType = f.type || 'text';
      }
      wrap.appendChild(input);
      appendHint(wrap, f);
      host.appendChild(wrap);
    }

    const ok = $('dialogOk');
    ok.textContent = okText;
    ok.className = danger ? 'ghost danger' : '';
    $('dialogOverlay').classList.add('show');
    (host.querySelector('input[type="text"], textarea, select') || ok).focus();
  });
}

function closeDialog(result) {
  $('dialogOverlay').classList.remove('show');
  // Stop any comparison preview that was playing inside the dialog.
  for (const a of document.querySelectorAll('#dialogFields audio')) a.pause();
  const resolve = dialogResolve;
  dialogResolve = null;
  if (resolve) resolve(result);
}

$('dialogOk').onclick = () => {
  const values = {};
  for (const el of document.querySelectorAll('#dialogFields [data-field]')) {
    if (el.dataset.fieldType === 'file') {
      values[el.dataset.field] = el.files[0] || null;
    } else {
      values[el.dataset.field] = el.value.trim();
    }
  }
  closeDialog({ ok: true, values });
};
$('dialogCancel').onclick = () => closeDialog({ ok: false });
$('dialogOverlay').onclick = (e) => {
  if (e.target === $('dialogOverlay')) closeDialog({ ok: false });
};
document.addEventListener('keydown', (e) => {
  if (dialogResolve && e.key === 'Escape') closeDialog({ ok: false });
});

async function confirmAction(message, { title, okText, danger = true } = {}) {
  title = title ?? (t("ui.please_confirm"));
  okText = okText ?? t('dialog.ok');
  const res = await showDialog({ title, message, okText, danger });
  return !!(res && res.ok);
}

/// Accept-Language so server messages come back in the UI language.
function apiFetch(url, opts = {}) {
  return fetch(url, {
    ...opts,
    headers: { 'Accept-Language': lang === 'zh' ? 'zh-CN' : 'en', ...(opts.headers || {}) },
  });
}

async function api(path, opts) {
  const res = await apiFetch(path, opts);
  if (!res.ok) {
    const body = await res.json().catch(() => ({}));
    throw new Error(body.error || res.statusText || ('HTTP ' + res.status));
  }
  const type = res.headers.get('content-type') || '';
  return type.includes('application/json') ? res.json() : res.text();
}
