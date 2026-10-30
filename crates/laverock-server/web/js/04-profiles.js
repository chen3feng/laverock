// ---------------------------------------------------------------- profiles

function selectedProfile() {
  return profiles.find((p) => p.id === $('profile').value) || null;
}

function currentEngine() {
  return engines.find((x) => x.id === $('engine').value) || null;
}

// Visibility of the voice/reference cards is driven by engine capability:
// preset-only engines (Kokoro) never show them. For cloning engines a
// selected saved voice replaces the ad-hoc reference recorder.
function syncVisibility() {
  const isClone = engineUsesClone(currentEngine());
  const p = selectedProfile();
  $('voicePickCard').hidden = !isClone;
  $('adhoc').hidden = !isClone || !!p;
}

function syncProfileUI() {
  const p = selectedProfile();
  syncVisibility();
  $('preview').disabled = !p;
  $('preview').textContent = previewId && p && previewId === p.id ? t('stop') : t('preview');
  $('profilehint').textContent = p
    ? t('ui.using_voice', { name: p.name, transcript: p.prompt_text })
    : t('profilehint.default');
}
