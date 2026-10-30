// ---------------------------------------------------------------- batch export

function openExportDialog() {
  if (!profiles.length) {
    toast(t("ui.no_voices_to_export"), true);
    return;
  }
  const list = $('exportList');
  list.innerHTML = '';
  // All checked by default.
  const rows = profiles.map((p) => {
    const label = document.createElement('label');
    label.style.cssText = 'display:flex;gap:8px;align-items:center;padding:6px 8px;border-radius:8px';
    label.dataset.name = p.name.toLowerCase();
    label.innerHTML = `<input type="checkbox" data-id="${esc(p.id)}" checked />
      <span style="flex:1">${esc(p.name)}</span>
      <span class="hint">${fmtDur(p.duration_secs)} · ${fmtBytes(p.bytes)}</span>`;
    return label;
  });
  rows.forEach((r) => list.appendChild(r));

  $('exportSearch').value = '';
  $('exportTitle').textContent = t("ui.export_voices");
  $('exportSearch').placeholder = t('voice.search');
  $('exportSelectAllLabel').textContent = t('voice.selectAll');
  $('exportCancel').textContent = t('dialog.cancel');
  $('exportOk').textContent = t('voice.exportSelected');
  const updateCount = () => {
    const n = list.querySelectorAll('input:checked').length;
    $('exportCount').textContent = t('voice.exportCount').replace('{n}', n);
    $('exportOk').disabled = n === 0;
    $('exportSelectAll').checked = n === rows.length && rows.length > 0;
    $('exportSelectAll').indeterminate = n > 0 && n < rows.length;
  };
  list.onchange = updateCount;
  $('exportSearch').oninput = () => {
    const q = $('exportSearch').value.trim().toLowerCase();
    list.querySelectorAll('label').forEach((l) => {
      l.style.display = !q || l.dataset.name.includes(q) ? '' : 'none';
    });
  };
  $('exportSelectAll').onchange = () => {
    const on = $('exportSelectAll').checked;
    list.querySelectorAll('label:not([style*="none"]) input').forEach((cb) => { cb.checked = on; });
    updateCount();
  };
  updateCount();
  $('exportDialog').classList.add('show');
}

function closeExportDialog() {
  $('exportDialog').classList.remove('show');
}

$('exportVoicesBtn').onclick = openExportDialog;
$('exportCancel').onclick = closeExportDialog;
$('exportDialog').addEventListener('click', (e) => {
  if (e.target.id === 'exportDialog') closeExportDialog();
});
$('exportOk').onclick = async () => {
  const ids = [...$('exportList').querySelectorAll('input:checked')].map((cb) => cb.dataset.id);
  if (!ids.length) return;
  const btn = $('exportOk');
  btn.disabled = true;
  try {
    const res = await apiFetch('/api/profiles/export', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ids }),
    });
    if (!res.ok) {
      const body = await res.json().catch(() => ({}));
      throw new Error(body.error || res.statusText);
    }
    const blob = await res.blob();
    const stamp = new Date().toISOString().slice(0, 10).replace(/-/g, '');
    await saveBlob(blob, `voices-${stamp}.lvvoices.zip`);
    closeExportDialog();
    toast(t("ui.voices_exported"));
  } catch (e) {
    toast((t("ui.export_failed")) + e.message, true);
    btn.disabled = false;
  }
};
