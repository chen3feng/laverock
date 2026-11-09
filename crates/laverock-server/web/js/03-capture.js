// ---------------------------------------------------------------- capture

class Capture {
  constructor(host, {
    autoTranscribe = true,
    transcribe = true,
    detailed = false,
    live = false,
    autoTranscribeFiles = false,
    model = null,
    liveModel = null,
  } = {}) {
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
    // Guards the async decode when the user picks a second file before
    // the first one finishes converting.
    this.loadSeq = 0;
    // Voice conversion records source audio, not a reference clip: there
    // is nothing to transcribe.
    this.autoTranscribe = autoTranscribe;
    this.showTranscribe = transcribe;
    // Detailed mode returns timed cues (and always records in history);
    // the plain endpoint is enough for filling a reference's text.
    this.detailed = detailed;
    // Live mode decodes the current utterance while recording, so text
    // appears with sentence-level latency instead of after the stop.
    this.live = live;
    // Pages with no separate transcribe button start as soon as a file is
    // chosen; the explicit click is then unnecessary.
    this.autoTranscribeFiles = autoTranscribeFiles;
    // Installed STT model id for final passes, and the one used for live
    // partials (the fast model when both are installed). Null = default.
    this.model = model;
    this.liveModel = liveModel;
    this.liveState = null;
    this.onChange = () => {};
    this.onTranscribed = () => {};
    // Detailed transcription streams; each finished cue is reported here
    // while the request is still running.
    this.onProgress = () => {};
    // Live partial/final text while recording.
    this.onLiveText = () => {};

    this.pick.onclick = () => this.file.click();
    this.transBtn.onclick = () => this.transcribe({ record: true });
    this.file.onchange = () => {
      const f = this.file.files[0];
      if (f) this.loadFile(f);
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
      this.transBtn.hidden = !this.showTranscribe;
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
      const res = await apiFetch('/api/transcribe' + modelQuery(this.model), {
        method: 'POST',
        body: fd,
      });
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
    const res = await apiFetch('/api/transcribe/stream' + modelQuery(this.model), {
      method: 'POST',
      body: fd,
    });
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

  // Take the picked file as the current clip. WAV uploads pass through;
  // anything else the browser can decode (MP3, M4A/AAC, FLAC, Ogg) is
  // converted here to mono 24 kHz WAV — the same shape as a microphone
  // recording — so the server keeps its single WAV parser.
  async loadFile(f) {
    const seq = (this.loadSeq += 1);
    this.blob = null;
    this.filename = null;
    this.fileLabel = '';
    this.setInfo(`${f.name} · ${fmtBytes(f.size)}`);
    this.syncTranscribeButton();
    this.onChange();
    let blob = f;
    try {
      if (!(await isWavFile(f))) {
        this.setInfo(`${t('cap.decoding')} ${f.name}`);
        blob = await decodeToWav(f);
      }
    } catch (_) {
      if (this.loadSeq !== seq) return;
      this.file.value = '';
      this.setInfo(t('cap.none'));
      toast(t('cap.decode_failed'), true);
      return;
    }
    if (this.loadSeq !== seq) return;
    this.blob = blob;
    this.filename = f.name;
    this.fileLabel = `${f.name} · ${fmtBytes(f.size)}`;
    this.setInfo(this.fileLabel);
    this.syncTranscribeButton();
    this.onChange();
    if (this.autoTranscribeFiles && transcribeAvailable) {
      this.transcribe({ record: true });
    }
  }

  clear() {
    if (this.recording) this.stop({ autoTranscribe: false });
    // Drop an in-flight file decode so it cannot restore the clip.
    this.loadSeq += 1;
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
    node.onaudioprocess = (e) => {
      const data = new Float32Array(e.inputBuffer.getChannelData(0));
      chunks.push(data);
      if (this.liveState) this.feedLive(data);
    };
    src.connect(node);
    node.connect(ctx.destination);

    this.stream = stream;
    this.ctx = ctx;
    this.srcNode = src;
    this.node = node;
    this.chunks = chunks;
    this.ratio = ctx.sampleRate / TARGET;
    if (this.live) {
      this.liveState = {
        utterance: [],
        length: 0,
        text: '',
        busy: false,
        lastLen: 0,
        speech: 0,
        silent: 0,
        pending: false,
        timer: setInterval(() => {
          if (this.liveState) this.pumpLive(this.liveState.pending);
        }, 1200),
      };
      // Drop the previous transcript while the new one is being spoken.
      this.onLiveText('');
    }
    this.recording = true;
    this.rec.classList.add('on');
    this.rec.textContent = t('cap.stop');
    this.setInfo(t("ui.recording_click_again_to_stop"));
  }

  // ------------------------------------------------------------ live text
  //
  // While recording, the current utterance is decoded every ~1.2 s and
  // shown as partial text. A pause finalizes it into the transcript, so
  // the delay stays at sentence level even though the model is offline.

  feedLive(data) {
    const st = this.liveState;
    const rate = this.ctx.sampleRate;
    st.utterance.push(data);
    st.length += data.length;
    let sum = 0;
    for (let i = 0; i < data.length; i++) sum += data[i] * data[i];
    const rms = Math.sqrt(sum / data.length);
    if (rms < LIVE_SILENCE_RMS) {
      st.silent += data.length;
    } else {
      st.silent = 0;
      st.speech += data.length;
    }
    const paused =
      st.silent >= rate * LIVE_PAUSE_SECS && st.speech >= rate * LIVE_MIN_SPEECH_SECS;
    const tooLong = st.length >= rate * LIVE_MAX_UTTERANCE_SECS;
    if (paused || tooLong) {
      st.pending = true;
      if (!st.busy) this.pumpLive(true);
    }
  }

  async pumpLive(finalize) {
    const st = this.liveState;
    if (!st || st.busy) return;
    if (st.length < this.ctx.sampleRate * 0.4) {
      st.pending = false;
      return;
    }
    if (!finalize && st.length === st.lastLen) return;
    st.busy = true;
    st.pending = false;
    st.lastLen = st.length;
    const take = st.length;
    const snapshot = concatFloat(st.utterance);
    try {
      const text = await this.decodeLive(snapshot);
      if (this.liveState !== st) return;
      if (finalize) {
        if (text) st.text = joinCjk(st.text, text);
        // Audio recorded during the request starts the next utterance.
        dropSamples(st, take);
        st.speech = 0;
        st.silent = 0;
        st.lastLen = 0;
        this.onLiveText(st.text);
      } else if (text) {
        this.onLiveText(joinCjk(st.text, text));
      }
    } catch (_) {
      // A failed partial keeps the previous text; the pass after stop is
      // authoritative anyway.
    } finally {
      if (this.liveState === st) {
        st.busy = false;
        if (st.pending) this.pumpLive(true);
      }
    }
  }

  async decodeLive(samples) {
    const down = downsample(samples, this.ctx.sampleRate / 24000);
    const fd = new FormData();
    fd.append('audio', new Blob([encodeWav(down, 24000)], { type: 'audio/wav' }), 'live.wav');
    const model = this.liveModel || this.model;
    const res = await apiFetch('/api/transcribe' + modelQuery(model), {
      method: 'POST',
      body: fd,
    });
    if (!res.ok) {
      const body = await res.json().catch(() => ({}));
      throw new Error(body.error || res.statusText);
    }
    return (((await res.json()).text) || '').trim();
  }

  stopLive() {
    if (!this.liveState) return;
    clearInterval(this.liveState.timer);
    this.liveState = null;
  }

  stop({ autoTranscribe = true } = {}) {
    if (!this.recording) return;
    this.recording = false;
    // A tap of the record button should not start a decode of silence.
    const live = this.liveState;
    const spoke = live
      ? live.speech > this.ctx.sampleRate * LIVE_MIN_SPEECH_SECS
      : false;
    this.stopLive();
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
    if (autoTranscribe && transcribeAvailable) {
      // Live dictation already produced text; the pass after stop refines
      // it with cues and keeps the result in history.
      if (this.live && (spoke || (live && live.text))) this.transcribe({ record: true });
      else if (this.autoTranscribe) this.transcribe();
    }
  }
}

// Live dictation: decode the current utterance periodically, finalize it
// after this much silence, and force a break for very long speech.
const LIVE_SILENCE_RMS = 0.0056;
const LIVE_PAUSE_SECS = 0.6;
const LIVE_MIN_SPEECH_SECS = 0.5;
const LIVE_MAX_UTTERANCE_SECS = 20;

// Drop the first `count` samples from the utterance buffers.
function dropSamples(st, count) {
  let left = count;
  while (left > 0 && st.utterance.length) {
    const first = st.utterance[0];
    if (first.length <= left) {
      left -= first.length;
      st.utterance.shift();
    } else {
      st.utterance[0] = first.subarray(left);
      left = 0;
    }
  }
  st.length = Math.max(0, st.length - count);
}

// Query suffix naming an installed STT model; empty keeps the default.
function modelQuery(model) {
  return model ? '?model=' + encodeURIComponent(model) : '';
}

// Join two text pieces: CJK needs no space, Latin words do.
function joinCjk(a, b) {
  a = (a || '').trim();
  b = (b || '').trim();
  if (!a) return b;
  if (!b) return a;
  const cjk = (c) => /[　-〿぀-ヿ㐀-䶿一-鿿豈-﫿＀-￯]/.test(c);
  return a + (!cjk(a[a.length - 1]) && !cjk(b[0]) ? ' ' : '') + b;
}

function concatFloat(arrs) {
  const n = arrs.reduce((a, b) => a + b.length, 0);
  const out = new Float32Array(n);
  let o = 0;
  for (const a of arrs) { out.set(a, o); o += a.length; }
  return out;
}
// Sniff the RIFF/WAVE header instead of trusting the extension: a
// mislabeled file would otherwise reach the server's WAV parser.
function isWavFile(file) {
  return file
    .slice(0, 12)
    .arrayBuffer()
    .then((buf) => {
      if (buf.byteLength < 12) return false;
      const v = new DataView(buf);
      return (
        v.getUint32(0, false) === 0x52494646 && // RIFF
        v.getUint32(8, false) === 0x57415645 // WAVE
      );
    })
    .catch(() => false);
}

// Decode any browser-supported audio container to mono 24 kHz WAV. An
// OfflineAudioContext needs no audio device and is not affected by
// autoplay policy; browsers decode into its sample rate where they can.
async function decodeToWav(file) {
  const Offline = window.OfflineAudioContext || window.webkitOfflineAudioContext;
  const Ctx = Offline || window.AudioContext || window.webkitAudioContext;
  if (!Ctx) throw new Error('AudioContext is unavailable');
  const ctx = Offline ? new Offline(1, 1, 24000) : new Ctx();
  let audio;
  try {
    audio = await ctx.decodeAudioData(await file.arrayBuffer());
  } finally {
    try { ctx.close().catch(() => {}); } catch (_) { /* already closed */ }
  }
  const channels = audio.numberOfChannels;
  const mono = new Float32Array(audio.length);
  for (let c = 0; c < channels; c++) {
    const data = audio.getChannelData(c);
    for (let i = 0; i < data.length; i++) mono[i] += data[i] / channels;
  }
  // Match the recording pipeline; lower-rate sources keep their rate
  // rather than being upsampled.
  const sr = Math.min(audio.sampleRate, 24000);
  const out = sr === audio.sampleRate ? mono : downsample(mono, audio.sampleRate / sr);
  return new Blob([encodeWav(out, sr)], { type: 'audio/wav' });
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
