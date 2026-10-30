// ---------------------------------------------------------------- import strategy

function chooseImportStrategy() {
  return new Promise((resolve) => {
    $('importTitle').textContent = t("ui.import_voices");
    $('importMessage').textContent = t('voice.importStrategy');
    const set = (id, key, danger) => {
      const b = $(id);
      b.textContent = t(key);
      b.className = danger ? 'danger' : (id === 'importSkip' ? '' : 'ghost');
    };
    set('importSkip', 'voice.strategySkip', false);
    set('importCopy', 'voice.strategyCopy', false);
    set('importOverwrite', 'voice.strategyOverwrite', true);
    $('importCancel').textContent = t('dialog.cancel');
    const overlay = $('importDialog');
    overlay.classList.add('show');
    const done = (v) => {
      overlay.classList.remove('show');
      resolve(v);
    };
    $('importSkip').onclick = () => done('skip');
    $('importCopy').onclick = () => done('copy');
    $('importOverwrite').onclick = () => done('overwrite');
    $('importCancel').onclick = () => done(null);
  });
}
$('newVoiceBtn').onclick = () => {
  const card = $('newVoiceCard');
  card.hidden = !card.hidden;
  if (!card.hidden) $('newName').focus();
};
$('newCancel').onclick = () => {
  $('newVoiceCard').hidden = true;
  $('newSaveHint').textContent = '';
};
$('newSave').onclick = async () => {
  const hint = $('newSaveHint');
  hint.className = 'hint';
  try {
    const meta = await createProfile(
      $('newName').value.trim(),
      $('newPromptText').value.trim(),
      voiceCapture.blob,
      $('engine').value,
    );
    $('newName').value = '';
    $('newPromptText').value = '';
    voiceCapture.clear();
    $('newVoiceCard').hidden = true;
    await loadProfiles();
    toast(t('ui.saved_voice', { name: meta.name }));
  } catch (e) {
    hint.className = 'hint err';
    hint.textContent = (t("ui.save_failed")) + e.message;
  }
};
