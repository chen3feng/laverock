// ---------------------------------------------------------------- remote / pairing

async function loadSession() {
  try {
    session = await api('/api/session');
  } catch (_) {
    session = { loopback: true, paired: false };
  }
  // Language preference: explicit local choice, then the machine-wide
  // server default, then the browser language.
  const explicit = localStorage.getItem('laverock.lang');
  if (!explicit && (session.language === 'zh' || session.language === 'en')) {
    if (lang !== session.language) {
      lang = session.language;
      applyI18n();
      fillPreset($('hfPreset'), HF_PRESETS);
      fillPreset($('pipPreset'), PIP_PRESETS);
    }
  }
  $('langSelect').value = explicit === 'zh' || explicit === 'en' ? explicit : 'system';
  $('settingsLang').value = $('langSelect').value;
  const desktop = !!session.loopback;
  $('remoteCard').hidden = !desktop;
  $('phoneSettings').hidden = desktop;

  if (!desktop && !session.paired) {
    // A QR link carries ?pair=CODE: claim it and reload without the query.
    const code = new URLSearchParams(location.search).get('pair');
    if (code) {
      $('pairCode').value = code;
      $('pairMsg').textContent = t("ui.pairing");
      if (await claim(code)) return;
    }
    $('pairOverlay').classList.add('show');
    return;
  }
  $('pairOverlay').classList.remove('show');

  if (desktop) {
    $('lanToggle').checked = !!session.lan_enabled;
    $('lanPanel').hidden = !session.lan_enabled;
    if (session.lan_enabled) {
      $('lanHint').textContent = t('ui.lan_url', { url: session.lan_url || '' });
      await loadDevices();
    }
  } else {
    $('phoneInfo').textContent = t('ui.connected_computer', {
      device: session.device_name ? t('ui.device_paren', { name: session.device_name }) : '',
    });
  }
}

async function claim(code) {
  const status = $('pairStatus');
  status.textContent = '';
  try {
    const res = await apiFetch('/api/pair/claim', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ code, device_name: deviceName() }),
    });
    if (!res.ok) {
      const msg = await res.json().then((j) => j.error).catch(() => res.statusText);
      throw new Error(msg);
    }
    location.replace('/');
    return true;
  } catch (e) {
    $('pairMsg').textContent = (t("ui.pairing_failed")) + e.message;
    $('pairOverlay').classList.add('show');
    return false;
  }
}

function deviceName() {
  const ua = navigator.userAgent;
  if (/iPhone/.test(ua)) return 'iPhone';
  if (/iPad/.test(ua)) return 'iPad';
  if (/Android/.test(ua)) return t("ui.android_phone");
  if (/Macintosh/.test(ua)) return t("ui.mac_browser");
  return t("ui.browser_device");
}

$('pairSubmit').onclick = () => {
  const code = $('pairCode').value.trim();
  if (!/^\d{6}$/.test(code)) {
    $('pairMsg').textContent = t("ui.enter_the_6_digit_pairing");
    return;
  }
  claim(code);
};

$('pairCode').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') $('pairSubmit').click();
});

$('unpair').onclick = () => {
  document.cookie = 'laverock_device=; Path=/; Max-Age=0';
  location.reload();
};

$('lanToggle').onchange = async () => {
  const enabled = $('lanToggle').checked;
  $('lanHint').textContent = enabled
    ? (t("ui.enabling"))
    : (t("ui.disabling"));
  const res = await apiFetch('/api/settings/lan', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ enabled }),
  });
  const body = await res.json().catch(() => ({}));
  if (!res.ok) {
    $('lanToggle').checked = !enabled;
    $('lanHint').textContent = (t("ui.operation_failed")) +
      body.error || res.statusText;
    return;
  }
  $('lanPanel').hidden = !enabled;
  $('qr').hidden = true;
  $('pairInfo').textContent = '';
  $('lanHint').textContent = enabled
    ? `${body.url || ''}${body.fingerprint ? (t("ui.cert_fingerprint")) + body.fingerprint.slice(0, 17) + '…' : ''}`
    : t('lan.hint.default');
  if (!enabled) $('deviceList').textContent = t('devices.none');
  else loadDevices();
};

$('pairBtn').onclick = async () => {
  $('pairInfo').textContent = t("ui.generating");
  const res = await apiFetch('/api/pair/start', { method: 'POST' });
  const body = await res.json().catch(() => ({}));
  if (!res.ok) {
    $('pairInfo').textContent = (t("ui.failed")) + body.error || res.statusText;
    return;
  }
  $('qr').innerHTML = body.qr_svg;
  $('qr').hidden = false;
  $('pairInfo').textContent = `${t("ui.pairing_code")} ${body.code}`;
  $('pairHint').textContent = en
    ? 'Scan the code with your phone to open the https link. The first visit warns about an untrusted certificate; choose "Advanced → Proceed" to allow microphone access (cert fingerprint '
      + (body.fingerprint ? body.fingerprint.slice(0, 17) + '…' : '') + '). Valid for 5 minutes.'
    : '手机扫码即可打开 https 链接；首次访问会提示证书不受信任，请选择「高级 → 继续前往」以允许使用麦克风（证书指纹 '
      + (body.fingerprint ? body.fingerprint.slice(0, 17) + '…' : '') + '）。5 分钟内有效。';
};

async function loadDevices() {
  let list = [];
  try { list = await api('/api/devices'); } catch (_) { list = []; }
  const box = $('deviceList');
  if (!list.length) { box.textContent = t('devices.none'); return; }
  box.innerHTML = '';
  for (const d of list) {
    const row = document.createElement('div');
    row.className = 'prompt-box';
    row.style.marginTop = '8px';
    const name = document.createElement('span');
    name.className = 'fileinfo';
    name.textContent = `${d.name} · ${new Date(d.created_at * 1000).toLocaleDateString(dateLocale())}`;
    const btn = document.createElement('button');
    btn.className = 'ghost small';
    btn.type = 'button';
    btn.textContent = t("ui.revoke");
    btn.onclick = async () => {
      const yes = await confirmAction(
        t('ui.revoke_confirm', { name: d.name }),
        { title: t("ui.revoke_device"), okText: t("ui.revoke") },
      );
      if (!yes) return;
      try {
        await api(`/api/devices/${encodeURIComponent(d.id)}`, { method: 'DELETE' });
        loadDevices();
      } catch (e) {
        toast((t("ui.revoke_failed")) + e.message, true);
      }
    };
    row.append(name, btn);
    box.appendChild(row);
  }
}
