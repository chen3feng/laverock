// ---------------------------------------------------------------- capture

class Capture {
  constructor(host, { autoTranscribe = true, transcribe = true, detailed = false } = {}) {
    host.innerHTML = '';
    host.appendChild($('captureTpl').content.cloneNode(true));
    applyI18n(host);
    this.host = host;
    this.file = host.querySelector('.cap-file');
    this.pick = host.querySelector('.cap-pick');
    this.rec = host.querySelector('.cap-rec');
    this.transBtn = host.querySelector('.cap-transcribe');
    this.info = host.querySelector('.cap-info');
    this.blob = null;
    this.filename = null;
    this.recording = false;
    this.transcribing = false;
    // Voice conversion records source audio, not a reference clip: there
    // is nothing to transcribe.
    this.autoTranscribe = autoTranscribe;
    this.showTranscribe = transcribe;
    // Detailed mode returns timed cues (and always records in history);
    // the plain endpoint is enough for filling a reference's text.
    this.detailed = detailed;
    this.onChange = () => {};
    this.onTranscribed = () => {};
    // Detailed transcription streams; each finished cue is reported here
    // while the request is still running.
    this.onProgress = () => {};

    this.pick.onclick = () => this.file.click();
    this.transBtn.onclick = () => this.transcribe({ record: true });
    this.file.onchange = () => {
      const f = this.file.files[0];
      if (!f) return;
      this.blob = f;
      this.filename = f.name;
      this.fileLabel = `${f.name} · ${fmtBytes(f.size)}`;
      this.setInfo(this.fileLabel);
      this.syncTranscribeButton();
      this.onChange();
    };
    this.rec.onclick = () => this.toggle();
  }

  syncTranscribeButton() {
    this.transBtn.hidden = !(this.showTranscribe && transcribeAvailable && this.blob && !this.transcribing);
  }

  setTranscribing(on) {
    this.transcribing = on;
    this.transBtn.disabled = on;
    this.syncTranscribeButton();
    if (on) {
      this.transBtn.hidden = false;
      this.transBtn.textContent = t('transcribe.busy');
      this.setInfo(t('transcribe.busy'));
    } else {
      this.transBtn.textContent = t('cap.transcribe');
    }
  }

  // Send the current clip to the on-device STT model. Resolves with the
  // text; null when nothing is selected or transcription is unavailable.
  // `record` keeps the result in history; automatic passes skip it.
  async transcribe({ record = false } = {}) {
    if (!this.blob || this.transcribing) return null;
    if (!transcribeAvailable) {
      toast(t("ui.install_the_sensevoice_speech_to"), true);
      return null;
    }
    this.setTranscribing(true);
    try {
      if (this.detailed) return await this.transcribeStream(record);
      const fd = new FormData();
      fd.append('audio', this.blob, this.filename || 'reference.wav');
      if (record) fd.append('record', '1');
      const res = await apiFetch('/api/transcribe', { method: 'POST', body: fd });
      if (!res.ok) {
        const body = await res.json().catch(() => ({}));
        throw new Error(body.error || res.statusText);
      }
      const body = await res.json();
      const text = body.text;
      if (!text) throw new Error(t("ui.no_speech_recognized"));
      this.onTranscribed(text, null);
      if (this.fileLabel) this.setInfo(this.fileLabel);
      return text;
    } catch (e) {
      if (this.fileLabel) this.setInfo(this.fileLabel);
      toast((t("ui.transcription_failed")) + e.message, true);
      return null;
    } finally {
      this.setTranscribing(false);
    }
  }

  // Detailed mode streams: cues arrive while the model works through the
  // clip, and the `done` event carries the finished transcript.
  async transcribeStream(record) {
    const fd = new FormData();
    fd.append('audio', this.blob, this.filename || 'reference.wav');
    if (record) fd.append('record', '1');
    const res = await apiFetch('/api/transcribe/stream', { method: 'POST', body: fd });
    if (!res.ok) {
      const body = await res.json().catch(() => ({}));
      throw new Error(body.error || res.statusText);
    }
    let transcript = null;
    let failure = null;
    await readSse(res, {
      segment: (segment) => this.onProgress(segment),
      done: (body) => { transcript = body; },
      error: (body) => {
        failure = new Error((body && body.message) || t('ui.transcription_failed'));
      },
    });
    if (failure) throw failure;
    if (!transcript || !transcript.text) {
      throw new Error(t('ui.no_speech_recognized'));
    }
    this.onTranscribed(transcript.text, transcript);
    if (this.fileLabel) this.setInfo(this.fileLabel);
    return transcript.text;
  }

  clear() {
    if (this.recording) this.stop({ autoTranscribe: false });
    this.blob = null;
    this.filename = null;
    this.fileLabel = '';
    this.file.value = '';
    this.setInfo(t('cap.none'));
    this.syncTranscribeButton();
    this.onChange();
  }

  setInfo(text) { this.info.textContent = text; }

  async toggle() {
    if (this.recording) return this.stop();
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
      this.setInfo(t("ui.microphone_recording_is_unavailable_here"));
      return;
    }
    this.setInfo(t("ui.requesting_microphone_permission"));
    let stream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({
        audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
      });
    } catch (e) {
      const name = (e && e.name) || 'Error';
      const hint = name === 'NotAllowedError'
        ? t('ui.mic_permission_denied')
        : name === 'NotFoundError'
          ? t('ui.mic_not_found')
          : t('ui.mic_check_settings');
      this.setInfo(`${t("ui.cannot_record")}（${name}）：${hint}`);
      return;
    }

    const ctx = new AudioContext();
    await ctx.resume();
    const src = ctx.createMediaStreamSource(stream);
    const TARGET = 24000;
    const node = ctx.createScriptProcessor(4096, 1, 1);
    const chunks = [];
    node.onaudioprocess = (e) => chunks.push(new Float32Array(e.inputBuffer.getChannelData(0)));
    src.connect(node);
    node.connect(ctx.destination);

    this.stream = stream;
    this.ctx = ctx;
    this.srcNode = src;
    this.node = node;
    this.chunks = chunks;
    this.ratio = ctx.sampleRate / TARGET;
    this.recording = true;
    this.rec.classList.add('on');
    this.rec.textContent = t('cap.stop');
    this.setInfo(t("ui.recording_click_again_to_stop"));
  }

  stop({ autoTranscribe = true } = {}) {
    if (!this.recording) return;
    this.recording = false;
    try { this.node.disconnect(); this.srcNode.disconnect(); } catch (_) { /* already closed */ }
    this.stream.getTracks().forEach((tr) => tr.stop());
    this.ctx.close();

    const raw = concatFloat(this.chunks);
    const down = downsample(raw, this.ratio);
    this.blob = new Blob([encodeWav(down, 24000)], { type: 'audio/wav' });
    this.filename = 'mic-reference.wav';
    this.fileLabel = `${t("ui.microphone_recording")} · ${(down.length / 24000).toFixed(1)}s`;
    this.setInfo(this.fileLabel);
    this.chunks = [];
    this.rec.classList.remove('on');
    this.rec.textContent = t('cap.record');
    this.syncTranscribeButton();
    this.onChange();
    if (autoTranscribe && this.autoTranscribe && transcribeAvailable) this.transcribe();
  }
}

function concatFloat(arrs) {
  const n = arrs.reduce((a, b) => a + b.length, 0);
  const out = new Float32Array(n);
  let o = 0;
  for (const a of arrs) { out.set(a, o); o += a.length; }
  return out;
}
function downsample(input, ratio) {
  const outLen = Math.floor(input.length / ratio);
  const out = new Float32Array(outLen);
  for (let i = 0; i < outLen; i++) {
    const start = Math.floor(i * ratio), end = Math.floor((i + 1) * ratio);
    let sum = 0;
    for (let j = start; j < end && j < input.length; j++) sum += input[j];
    out[i] = sum / Math.max(1, end - start);
  }
  return out;
}
function encodeWav(samples, sr) {
  const buf = new ArrayBuffer(44 + samples.length * 2);
  const v = new DataView(buf);
  const wstr = (o, s) => { for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i)); };
  wstr(0, 'RIFF'); v.setUint32(4, 36 + samples.length * 2, true); wstr(8, 'WAVE');
  wstr(12, 'fmt '); v.setUint32(16, 16, true); v.setUint16(20, 1, true);
  v.setUint16(22, 1, true); v.setUint32(24, sr, true);
  v.setUint32(28, sr * 2, true); v.setUint16(32, 2, true); v.setUint16(34, 16, true);
  wstr(36, 'data'); v.setUint32(40, samples.length * 2, true);
  let o = 44;
  for (const s of samples) {
    v.setInt16(o, Math.max(-1, Math.min(1, s)) * 0x7fff, true); o += 2;
  }
  return buf;
}

// Read a Server-Sent Events response, calling `handlers[event]` for each
// framed event with its parsed JSON payload.
async function readSse(res, handlers) {
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buf = '';
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
      const handler = handlers[event];
      if (!handler) continue;
      const data = dataLines.join('\n');
      handler(data ? JSON.parse(data) : null);
    }
  }
}

const synthCapture = new Capture($('synthCapture'));
synthCapture.onTranscribed = (text) => {
  $('prompt_text').value = text;
  saveDraft();
};
const voiceCapture = new Capture($('voiceCapture'));
voiceCapture.onTranscribed = (text) => {
  $('newPromptText').value = text;
};

if (!window.isSecureContext) {
  for (const cap of [synthCapture, voiceCapture]) {
    cap.rec.disabled = true;
    cap.rec.title = t("ui.microphone_access_requires_https_or");
  }
}
