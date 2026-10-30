# Voice quality

English | [中文](quality.zh-CN.md) · [← README](../README.md)

Speed tells you an engine is usable; this page tells you whether it is
*food*. Every number below is measured on-device, on this machine, with the
same reference clip and the same test sentences, then scored with small ONNX
models that ship no cloud dependency.

Companion to the [speed table](models.md#speed), which measures the same
engines for latency.

## What is measured

| Metric | Question it answers | How it is computed | How to read it |
| --- | --- | --- | --- |
| **CER** | Did it say the right words? | SenseVoice transcribes the output; character-level edit distance against the input text (also for en, so an ASR splitting "seashells" into "sea shells" does not count as a mistake) | 0 is perfect. One wrong character in a 10-character line is 0.10, so treat differences under ~0.05 as noise |
| **Hard-case CER** | Heteronyms, tongue twisters, numbers, acronyms, code-switching | Same, scored per case; see *Test material* below | This is where TTS slips even when plain sentences come out clean |
| **Repeat** | Did the decoder loop? | Share of repeated 4-grams (3-grams for en) in the transcript | > 0.05 means a phrase repeats |
| **Similarity** | Does it sound like the reference speaker? | CAM++ speaker embedding cosine between the output and the reference clip | Same speaker, different recording: ≈ 0.7–0.86. Different speaker: ≈ 0.2–0.4. Cloning engines only |
| **Take-to-take** | Is the same line reproducible? | Three takes of one sentence, cosine between their embeddings | 1.000 is deterministic; lower means the voice wanders between runs |
| **Drift (min)** | Does the voice hold up across a paragraph? | The long paragraph is cut into 3 s windows, each embedded; this is the worst window's cosine to the paragraph opening | Closer to 1.0 is steadier; below ~0.7 the voice audibly changes |
| **DNSMOS OVR / SIG** | How clean does it sound? | Microsoft DNSMOS P.835 predictor over 9 s windows | 1–5 scale. 3.0 is fair, 3.5 good, 4.0+ excellent |
| **Pause** | Is the pacing right? | Share of near-silent frames | 0.15–0.35 reads naturally; very low is breathless, very high is sluggish |
| **Pitch range** | Is it expressive or monotone? | p90−p10 of F0 in semitones | Read Chinese sits around 5–10; under ~3 sounds flat |
| **Chars/s** | Speaking rate over the audible part | Characters ÷ voiced seconds | Mandarin conversation is roughly 5–7 |

## Test material

| Group | Cases |
| --- | --- |
| Base | one short line ×3 (reproducibility), one normal sentence, one ~140-character paragraph (drift, pauses), one English sentence |
| Hard | `银行行长说，他重新开始了这段很长的旅程。` (heteronyms 行/长/重) · `四是四，十是十，十四是十四，四十是四十。` (sibilant tongue twister) · `订单金额 3800.5 元，日期是 2026 年 10 月 30 日，完成率 85%。` (decimals, dates, percent) · `这个 model 的 inference 速度很快，但 latency 还需要优化。` (zh–en code-switching) · `She sells sea shells by the sea shore.` · `The API returns an error when the JSON payload is too large.` (acronyms) |

Number-heavy lines are compared against their **spoken form** (`3800.5` →
`三千八百点五`, `85%` → `百分之八十五`), because an ASR hears the words, not
the digits. For those cases a different-but-valid reading still counts as an
error, which is why the transcripts themselves are printed in the report.

## Results

Measured 2026-10-30 on an Apple M4 Pro (14-core CPU / 20-core GPU, 48 GB,
macOS 27.0.1), on the default voice for non-cloning engines and one reference
clip for the cloning ones. Scored with SenseVoice-Small (ITN off), CAM++ and
DNSMOS.

### Overall

| Engine | Clone | CER zh | CER en | Repeat | Similarity | Take-to-take | Drift (min) | DNSMOS OVR | SIG | Pause | Pitch range (st) | Chars/s |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| chatterbox | yes | 0.029 | 0.000 | 0.006 | 0.82 | 0.902 | 0.07 | 3.21 | 3.50 | 0.33 | 8.6 | 8.4 |
| cosyvoice | yes | 0.002 | 0.000 | 0.000 | 0.88 | 1.000 | 0.85 | 3.22 | 3.48 | 0.31 | 5.4 | 6.9 |
| dots | yes | 0.002 | 0.000 | 0.013 | 0.78 | 1.000 | 0.85 | 3.10 | 3.46 | 0.24 | 4.7 | 7.5 |
| kokoro-int8-multi-lang-v1_0 | – | 0.008 | 0.000 | 0.000 | – | 0.972 | 0.87 | 3.34 | 3.58 | 0.16 | 7.1 | 7.9 |
| longcat | yes | 0.002 | 0.135 | 0.000 | 0.86 | 1.000 | 0.85 | 3.30 | 3.56 | 0.32 | 3.9 | 8.8 |
| ming3tts | yes | 0.002 | 0.000 | 0.000 | 0.81 | 0.814 | 0.89 | 3.15 | 3.40 | 0.26 | 5.1 | 8.2 |
| ming3tts-design | – | 0.002 | 0.000 | 0.009 | – | 0.750 | 0.90 | 3.25 | 3.50 | 0.33 | 11.7 | 8.8 |
| ming3tts-ip | – | 0.002 | 0.000 | 0.005 | – | 0.835 | 0.59 | 3.22 | 3.51 | 0.28 | 11.3 | 7.8 |
| ming3tts-multilingual | yes | 0.002 | 0.000 | 0.000 | 0.85 | 0.910 | 0.89 | 3.17 | 3.44 | 0.28 | 4.2 | 8.2 |
| mosstts | yes | 0.002 | 0.000 | 0.000 | 0.77 | 1.000 | 0.91 | 3.19 | 3.49 | 0.32 | 5.4 | 8.6 |
| qwen3tts | yes | 0.002 | 0.000 | 0.000 | 0.76 | 0.776 | 0.81 | 3.13 | 3.43 | 0.31 | 8.6 | 7.7 |
| qwen3tts-custom | – | 0.002 | 0.000 | 0.000 | – | 0.860 | 0.79 | 3.24 | 3.48 | 0.39 | 20.2 | 7.7 |
| qwen3tts-design | – | 0.002 | 0.000 | 0.000 | – | 0.778 | 0.78 | 3.28 | 3.52 | 0.32 | 7.7 | 8.8 |
| sherpa-onnx-zipvoice-distill-int8-zh-en-emilia | yes | 0.002 | 0.459 | 0.007 | 0.80 | 0.842 | 0.90 | 3.01 | 3.34 | 0.21 | 4.7 | 7.5 |
| voxcpm2 | yes | 0.002 | 0.000 | 0.000 | 0.76 | 1.000 | 0.90 | 3.11 | 3.39 | 0.33 | 4.3 | 8.7 |

### Hard cases (CER)

| Engine | heteronyms | tongue twister | numbers/dates | zh-en mix | en twister | en acronyms |
| --- | --- | --- | --- | --- | --- | --- |
| chatterbox | 0.000 | 0.375 | 0.364 | 0.265 | 0.000 | 0.042 |
| cosyvoice | 0.000 | 0.000 | 0.061 | 0.000 | 0.300 | 0.083 |
| dots | 0.000 | 0.250 | 0.000 | 0.000 | 0.367 | 0.000 |
| kokoro-int8-multi-lang-v1_0 | 0.056 | 1.688 | 0.818 | 0.735 | 0.000 | 0.000 |
| longcat | 0.000 | 0.062 | 0.333 | 0.147 | 0.400 | 0.062 |
| ming3tts | 0.000 | 0.062 | 0.182 | 0.000 | 0.000 | 0.062 |
| ming3tts-design | 0.000 | 0.312 | 0.121 | 0.000 | 0.000 | 0.000 |
| ming3tts-ip | 0.000 | 0.312 | 0.091 | 0.000 | 0.000 | 0.000 |
| ming3tts-multilingual | 0.000 | 0.062 | 0.121 | 0.000 | 0.000 | 0.062 |
| mosstts | 0.000 | 0.062 | 0.000 | 0.000 | 0.133 | 0.083 |
| qwen3tts | 0.000 | 0.000 | 0.000 | 0.000 | 0.067 | 0.000 |
| qwen3tts-custom | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 |
| qwen3tts-design | 0.000 | 0.000 | 0.000 | 0.029 | 0.000 | 0.000 |
| sherpa-onnx-zipvoice-distill-int8-zh-en-emilia | 0.000 | 0.062 | 2.364 | 0.235 | 0.700 | 0.500 |
| voxcpm2 | 0.000 | 0.062 | 0.000 | 0.000 | 0.000 | 0.000 |

### What stands out

- **qwen3tts (all three variants) and voxcpm2 are the only engines that come
  through every hard case clean** — no heteronym, number or acronym errors, at
  most one split word ("sea shells") or one character on the tongue twister.
- **chatterbox holds the voice worst.** It clones at 0.82, but its worst 3 s
  window in a paragraph drops to 0.07 — the voice changes mid-paragraph — and
  it garbles numbers (0.364, "三千八百点五" → "三间先险负五").
- **kokoro is an English engine.** Clean English, but on Chinese it produces
  English-sounding syllables (tongue twister 1.688, code-switching 0.735).
  Fine for its intended language, unusable for the other.
- **The tongue twister separates the Chinese models.** Most sit at 0.000–0.062
  (sibilants right), while chatterbox, dots and ming3tts-design/-ip land at
  0.25–0.38 by dropping or doubling 十/四.
- **Numbers are the sharpest divide.** zipvoice reads `3800.5` as an English
  phrase inside a Chinese sentence (2.364) and longcat drops the zeroes
  ("三八零零点五", "一零月三零日"); everyone else reads them correctly.
- **cosyvoice has the best clone similarity (0.88)** and is steady, but its
  English is weak: the sea-shells line becomes "she sells seeaws by the seas".
  longcat clones second best (0.86) with the worst English accuracy (0.135).
- **Two pacing outliers:** qwen3tts-custom spans 20 semitones of pitch
  (expressive or unstable depending on the line), longcat only 3.9 (flat);
  ming3tts-ip drifts to 0.59 across the paragraph — apart from chatterbox, the
  only engine below 0.7.

## How to read these numbers

- **The STT model runs with inverse text normalization off.** With ITN on,
  "三千八百点五" comes back as "3800.5", which scores as an error against a
  test sentence written in characters — a correct reading would be punished.

- **CER is an ASR round-trip, not a human judgement.** SenseVoice makes its
  own mistakes, more on names and numbers; a 0.02 gap between two engines is
  not meaningful, but a 0.10 is.
- **Similarity has a ceiling.** The reference is one recording of the speaker.
  A second recording of the same person against it scores about 0.7–0.86, so
  a clone at 0.75 is close to what the metric can distinguish. Above that,
  listen instead.
- **Take-to-take is partly a determinism check.** Engines with a fixed seed
  return byte-identical audio for the same input (1.000 by construction);
  for others it measures how much the voice moves between runs.
- **DNSMOS was trained on noise suppression, not on TTS.** It tracks
  background noise, artifacts and muffling; it does not know about unnatural
  prosody. Pair it with the pitch and pacing columns.
- Engines marked "–" for similarity do not clone: preset-voice and
  voice-design engines sing in their own voice, so the reference cosine would
  measure coincidence, not cloning.

## Reproducing

Any virtualenv with `numpy soundfile librosa onnxruntime` works; the script
downloads both scoring models on first use.

```bash
# 1. start the server with the engines installed
LAVEROCK_MODELS_DIR=... ./target/debug/laverock

# 2. synthesize and score
python3 scripts/eval-quality.py \
  --base http://127.0.0.1:8899 \
  --out /tmp/quality \
  --models /tmp/quality-models \
  --ref path/to/reference.wav \
  --ref-text "the transcript of that clip" \
  --report quality.md
```

`--engines id1,id2` limits the sweep; `--phase synth` or `--phase score`
splits the two halves; `--force` re-synthesizes clips that already exist.

Before step 1, turn ITN off in the STT engine's `engine.json`
(`"stt": { "use_itn": false }`) — or copy the models directory, change that one
field and point `LAVEROCK_MODELS_DIR` at the copy — since the server applies it
at load time. The same `--report` file lists every hard-case transcript, which
is what you read when a number looks wrong.

## Metrics worth adding next

Cheap to add, each answering something the current table cannot:

| Metric | What it would tell us | Cost |
| --- | --- | --- |
| UTMOS22 / NISQA | A TTS-trained MOS predictor, closer to human judgement than DNSMOS | one torch model, ~100 MB |
| MCD and F0-contour distance | Prosody fidelity to the reference, not just timbre | needs the same speaker reading the test set |
| Phoneme-level alignment | *Which* sounds fail on the hard cases, not just how many | MFA or a wav2vec2 aligner |
| Emotion adherence | Does "excited" sound excited? Measure F0/energy shift against a neutral take | a few more syntheses |
| Long-form stability | Chunk seams and drift over a 5-minute document | minutes per engine |
| Time-to-first-audio | What a streaming UI actually feels like | needs streaming support |
| Blind MOS / CMOS | The only thing that really settles the ranking | human listeners |

## What these numbers do not capture

- **Human preference.** MOS and CMOS still need ears; everything above is a
  proxy. A blind listening pass is the honest next step for the top few
  engines.
- **Prosody fidelity to the reference.** Matching *what* the speaker sounds
  like (this page) is easier than matching *how* they say a sentence —
  intonation contour, stress, rhythm. Measuring that needs a parallel
  recording: the same text read by the same person.
- **Emotion and style control.** Whether "excited" sounds excited is a
  different axis from whether it sounds like the reference.
- **Long-form behaviour.** The paragraph here is ~20 s; a five-minute
  document adds sentence-boundary artifacts, chunk seams and slow drift that
  a single paragraph does not reveal.
