//! One place for every user-facing message.
//!
//! Manifests and the catalog carry their own localized text ([`Localized`]),
//! but strings produced by Rust (validation errors, engine messages, the
//! progress stages the install scripts report) live here, keyed and
//! translated once instead of scattered `if lang == "en"` branches.
//!
//! Internal diagnostics stay English: only messages a user can see are
//! translated.

use std::fmt;

/// UI language. Unknown codes resolve to English.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    Zh,
    #[default]
    En,
}

impl Lang {
    pub fn parse(code: &str) -> Self {
        if code.trim().to_ascii_lowercase().starts_with("zh") {
            Lang::Zh
        } else {
            Lang::En
        }
    }

    /// Resolve from an `Accept-Language`-style header (`zh-CN,zh;q=0.9`).
    pub fn from_accept_language(header: Option<&str>) -> Self {
        let Some(header) = header else {
            return Lang::En;
        };
        header
            .split(',')
            .next()
            .map(Lang::parse)
            .unwrap_or(Lang::En)
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }
}

macro_rules! messages {
    ($($variant:ident = $key:literal { en: $en:literal, zh: $zh:literal }),+ $(,)?) => {
        /// Every user-facing message, with both translations next to each
        /// other so a missing translation is a compile error.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Key {
            $($variant),+
        }

        impl Key {
            fn translations(self) -> (&'static str, &'static str) {
                match self {
                    $(Key::$variant => ($en, $zh)),+
                }
            }

            /// Stable identifier, also used in script progress markers.
            pub fn id(self) -> &'static str {
                match self {
                    $(Key::$variant => $key),+
                }
            }

            /// Key for a script-emitted identifier.
            pub fn from_id(id: &str) -> Option<Key> {
                match id {
                    $($key => Some(Key::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

messages! {
    // ---- engine input validation ----
    ReferenceAudioRequired = "reference_audio_required"
        { en: "{engine} needs a reference clip (3–10 s of clean speech works best).",
          zh: "{engine} 需要一段参考音频（3–10 秒干净人声效果最好）。" },
    TranscriptRequired = "transcript_required"
        { en: "{engine} needs the transcript of the reference clip.",
          zh: "{engine} 需要参考音频对应的文字。" },
    ZeroShotTranscriptRequired = "zero_shot_transcript_required"
        { en: "Zero-shot cloning needs the reference transcript; switch to the cross-lingual template if unsure.",
          zh: "零样本克隆需要参考音频对应的文字；不确定时可改用「跨语种」模板。" },
    InstructRequired = "instruct_required"
        { en: "The instruct template needs an instruction, e.g. \"read this gently\".",
          zh: "「指令」模板需要填写指令内容，例如「用温和的语气朗读」。" },
    VoiceDesignDescriptionRequired = "voice_design_required"
        { en: "Voice design needs a description of the voice.",
          zh: "声音设计模式需要填写音色描述（instruct）。" },
    QwenReferenceRequired = "qwen_reference_required"
        { en: "Qwen3-TTS clone mode needs a reference clip (3 s or longer works best).",
          zh: "Qwen3-TTS 克隆模式需要一段参考音频（3 秒以上效果更好）。" },
    MingReferenceRequired = "ming_reference_required"
        { en: "Ming-3.0-tts clone/multilingual mode needs a reference clip.",
          zh: "Ming-3.0-tts 克隆 / 多语模式需要一段参考音频。" },
    ChatterboxReferenceRequired = "chatterbox_reference_required"
        { en: "Chatterbox clones your voice, so it needs a reference clip.",
          zh: "Chatterbox 克隆你的声音，需要一段参考音频。" },
    EmptyClip = "empty_clip"
        { en: "The audio clip is empty.", zh: "音频为空。" },
    SttModelMissing = "stt_model_missing"
        { en: "No speech-to-text model installed; download SenseVoice from the Models page.",
          zh: "未安装语音识别模型；请在「模型」页下载 SenseVoice。" },
    SttFileMissing = "stt_file_missing"
        { en: "Speech-to-text model file missing: {path}",
          zh: "语音识别模型文件缺失：{path}" },
    SttEngineUnsupported = "stt_engine_unsupported"
        { en: "Unsupported speech-to-text engine: {engine}",
          zh: "不支持的语音识别引擎：{engine}" },

    // ---- voice library ----
    VoiceNameEmpty = "voice_name_empty"
        { en: "Voice name cannot be empty.", zh: "声音名称不能为空。" },
    VoiceNameTooLong = "voice_name_too_long"
        { en: "Voice name is too long (80 characters max).", zh: "声音名称过长（最多 80 字）。" },
    TranscriptMissing = "transcript_missing"
        { en: "The reference transcript cannot be empty.", zh: "参考音频对应的文字不能为空。" },
    ReferenceTooLarge = "reference_too_large"
        { en: "The reference audio exceeds 20 MB.", zh: "参考音频超过 20 MB。" },
    ReferenceSilent = "reference_silent"
        { en: "The reference audio is silent; record or upload a clip with speech.",
          zh: "参考音频几乎全是静音，请重新录制或上传一段有人声的音频。" },
    ArchiveNotZip = "archive_not_zip"
        { en: "Not a valid voice archive (not a ZIP): {error}", zh: "无效的音色文件（不是 ZIP）：{error}" },
    ArchiveUnreadable = "archive_unreadable"
        { en: "Cannot read the voice archive: {error}", zh: "无法读取音色文件：{error}" },
    ArchiveMissingIndex = "archive_missing_index"
        { en: "The voice archive is missing manifest.json / voices.json.",
          zh: "音色文件缺少 manifest.json / voices.json。" },
    ArchiveMissingWav = "archive_missing_wav"
        { en: "The voice archive is missing reference.wav.", zh: "音色文件缺少 reference.wav。" },
    ArchiveBadFormat = "archive_bad_format"
        { en: "Not a laverock voice archive (format mismatch).", zh: "不是 laverock 音色文件（format 不匹配）。" },
    ArchiveNoVoices = "archive_no_voices"
        { en: "The archive contains no voices.", zh: "音色文件中没有声音。" },
    ArchiveVoicesMissing = "archive_voices_missing"
        { en: "voices.json has no voices array.", zh: "voices.json 缺少 voices 数组。" },
    NothingSelected = "nothing_selected"
        { en: "No voices selected.", zh: "未选择任何声音。" },

    // ---- pairing / remote ----
    PairNoSession = "pair_no_session"
        { en: "No pairing in progress.", zh: "没有进行中的配对。" },
    PairExpired = "pair_expired"
        { en: "The pairing code has expired; generate a new one on the computer.",
          zh: "配对码已过期，请在电脑上重新生成。" },
    PairTooMany = "pair_too_many_attempts"
        { en: "Too many attempts; generate a new pairing code.", zh: "尝试次数过多，请重新生成配对码。" },
    PairInvalid = "pair_invalid"
        { en: "Incorrect pairing code.", zh: "配对码不正确。" },
    DeviceUnpaired = "device_unpaired"
        { en: "This device is not paired; scan the computer's QR code first.",
          zh: "未配对的设备，请先在电脑上扫码配对。" },
    LoopbackOnly = "loopback_only"
        { en: "This action is only available on the computer.", zh: "该操作只能在电脑端进行。" },
    LanUnavailable = "lan_unavailable"
        { en: "Enable LAN access first and make sure the computer is online.",
          zh: "请先开启局域网访问，并确认电脑已连接到网络。" },

    // ---- models ----
    UnknownModel = "unknown_model"
        { en: "Unknown model: {model}", zh: "未知的模型：{model}" },
    AlreadyInstalled = "already_installed"
        { en: "{model} is already installed.", zh: "{model} 已经安装。" },
    DownloadInProgress = "download_in_progress"
        { en: "This model is already downloading.", zh: "该模型正在下载中。" },
    ScriptsDirMissing = "scripts_dir_missing"
        { en: "Download scripts directory not found. Set LAVEROCK_SCRIPTS_DIR to the repository's scripts/ directory.",
          zh: "找不到下载脚本目录。请设置 LAVEROCK_SCRIPTS_DIR 指向仓库的 scripts/ 目录。" },
    UnsupportedPlatform = "unsupported_platform"
        { en: "{model} is not supported on this platform; see the model notes for details.",
          zh: "当前平台不支持 {model}，原因见该模型的说明。" },
    ScriptMissing = "script_missing"
        { en: "Download script missing: {path}", zh: "缺少下载脚本：{path}" },
    ManifestReadFailed = "manifest_read_failed"
        { en: "Cannot read engine.json: {error}", zh: "读取 engine.json 失败：{error}" },
    ManifestParseFailed = "manifest_parse_failed"
        { en: "engine.json cannot be parsed: {error}", zh: "engine.json 无法解析：{error}" },

    // ---- settings ----
    ManualProxyRequired = "manual_proxy_required"
        { en: "Manual proxy mode needs a proxy URL, or switch back to the system proxy.",
          zh: "手动代理模式需要填写代理地址，或改用「跟随系统代理」。" },
    UrlSchemeRequired = "url_scheme_required"
        { en: "{label} must start with http:// or https://.", zh: "{label}需要以 http:// 或 https:// 开头。" },
    UrlSchemeUnsupported = "url_scheme_unsupported"
        { en: "{label} does not support {scheme}://; allowed: {allowed}.",
          zh: "{label}不支持 {scheme}://，可用：{allowed}。" },
    UrlHostMissing = "url_host_missing"
        { en: "{label} is missing a host name.", zh: "{label}缺少主机名。" },
    LabelProxy = "label_proxy"
        { en: "Proxy", zh: "代理" },
    LabelHfMirror = "label_hf_mirror"
        { en: "Hugging Face mirror", zh: "Hugging Face 镜像" },
    LabelPipMirror = "label_pip_mirror"
        { en: "PyPI mirror", zh: "PyPI 镜像" },
    LabelWorkspace = "label_workspace"
        { en: "Workspace directory", zh: "工作区目录" },
    WorkspaceDirNotAbsolute = "workspace_dir_not_absolute"
        { en: "{label} must be an absolute path, e.g. /data/laverock or D:\\laverock.",
          zh: "{label}必须是绝对路径，例如 /data/laverock 或 D:\\laverock。" },
    UnsupportedLanguage = "unsupported_language"
        { en: "Unsupported language: {lang}", zh: "不支持的语言：{lang}" },

    // ---- export ----
    Mp3TooLarge = "mp3_too_large"
        { en: "Audio is too large to export as MP3.", zh: "音频过大，无法导出 MP3。" },
    WavNoAudio = "wav_no_audio"
        { en: "The WAV contains no audio to encode.", zh: "WAV 中没有可编码的音频数据。" },
    WavUnreadable = "wav_unreadable"
        { en: "Could not parse the WAV audio.", zh: "无法解析 WAV 音频。" },
    Mp3EncodeFailed = "mp3_encode_failed"
        { en: "MP3 encoding failed.", zh: "MP3 编码失败。" },

    // ---- install script stages (emitted as ids by download-*.sh) ----
    StageWaiting = "stage_waiting"
        { en: "Waiting to start", zh: "等待开始" },
    StageDownloadFile = "stage_download_file"
        { en: "Downloading {name} ({size})", zh: "下载 {name}（{size}）" },
    StageDownloadArchive = "stage_download_archive"
        { en: "Downloading {name}", zh: "下载 {name}" },
    StageExtract = "stage_extract"
        { en: "Extracting model files", zh: "解压模型文件" },
    StageWriteManifest = "stage_write_manifest"
        { en: "Writing engine manifest", zh: "写入引擎清单" },
    StageVenv = "stage_venv"
        { en: "Creating Python virtual environment", zh: "创建 Python 虚拟环境" },
    StagePipUpgrade = "stage_pip_upgrade"
        { en: "Upgrading pip", zh: "升级 pip" },
    StagePipInstall = "stage_pip_install"
        { en: "Installing dependencies", zh: "安装依赖" },
    StageTorchCuda = "stage_torch_cuda"
        { en: "Installing CUDA PyTorch (~1–2 GB the first time)", zh: "安装 CUDA 版 PyTorch（首次约 1–2 GB）" },
    StageTorch = "stage_torch"
        { en: "Installing PyTorch (torch + torchaudio)", zh: "安装 PyTorch（torch + torchaudio）" },
    StageAudioCpp = "stage_audiocpp"
        { en: "Downloading the audio.cpp runtime ({asset})", zh: "下载 audio.cpp 运行时（{asset}）" },
    StageCudaRuntime = "stage_cuda_runtime"
        { en: "Downloading the CUDA runtime (cudart {version})", zh: "下载 CUDA 运行库（cudart {version}）" },
    StageDownloading = "stage_downloading"
        { en: "Downloading {name}", zh: "下载 {name}" },
    StageResume = "stage_resume"
        { en: "Incomplete download; resuming (attempt {attempt})", zh: "下载不完整，正在续传（第 {attempt} 次）" },
    StageQwenDeps = "stage_qwen_deps"
        { en: "Installing qwen-tts dependencies (~2 GB the first time)", zh: "安装 qwen-tts 依赖（约 2 GB，首次较慢）" },
    StageInstallPipDeps = "stage_install_pip_deps"
        { en: "Installing Python dependencies (torch 2.6 + transformers, ~2 GB the first time)", zh: "安装 Python 依赖（torch 2.6 + transformers，首次约 2 GB）" },
    StageFetchRuntime = "stage_fetch_runtime"
        { en: "Fetching the engine Python runtime", zh: "拉取引擎 Python 运行库" },
    StageDownloadModelTokenizer = "stage_download_model_tokenizer"
        { en: "Downloading model and tokenizer (~2.5 GB the first time)", zh: "下载模型与分词器（首次约 2.5 GB）" },
    StageFetchListing = "stage_fetch_listing"
        { en: "Fetching the file list from HuggingFace ({base})", zh: "从 HuggingFace 获取文件列表（{base}）" },
    StageFetchSource = "stage_fetch_source"
        { en: "Fetching LongCat-AudioDiT source", zh: "拉取 LongCat-AudioDiT 源码" },
    StageInstallDeps = "stage_install_deps"
        { en: "Installing dependencies (PyTorch and transformers 5, ~1–2 GB)", zh: "安装依赖（PyTorch 与 transformers 5，约 1–2 GB）" },
    StageDownloadWeights = "stage_download_weights"
        { en: "Downloading model weights ({note})", zh: "下载模型权重（{note}）" },
    StageDownloadTokenizer = "stage_download_tokenizer"
        { en: "Downloading the UMT5 tokenizer", zh: "下载 UMT5 分词器" },
    StageInstallRunner = "stage_install_runner"
        { en: "Installing the runner script", zh: "安装运行脚本" },
    StageDownloadVocoder = "stage_download_vocoder"
        { en: "Downloading vocoder (2/3)", zh: "下载声码器（2/3）" },
    StageDownloadLexicon = "stage_download_lexicon"
        { en: "Downloading English lexicon (3/3)", zh: "下载英文词典（3/3）" },
    StageVerifyVox = "stage_verify_vox"
        { en: "Verifying the prebuilt voxcpm2-cli", zh: "校验 voxcpm2-cli 预编译版本" },
    StageVoxReady = "stage_vox_ready"
        { en: "Prebuilt voxcpm2-cli is ready", zh: "voxcpm2-cli 预编译版本就绪" },
    StageVerifyMoss = "stage_verify_moss"
        { en: "Verifying the prebuilt llama-moss-tts", zh: "校验 llama-moss-tts 预编译版本" },
    StageMossReady = "stage_moss_ready"
        { en: "Prebuilt llama-moss-tts is ready", zh: "llama-moss-tts 预编译版本就绪" },
    StageDownloadSize = "stage_download_size"
        { en: "Downloading {name} · {size}", zh: "下载 {name} · {size}" },
}

/// Text for `key` in `lang`.
pub fn text(lang: Lang, key: Key) -> &'static str {
    let (en, zh) = key.translations();
    match lang {
        Lang::Zh => zh,
        Lang::En => en,
    }
}

/// A translated message plus its `{placeholder}` arguments.
#[derive(Debug, Clone)]
pub struct Message {
    key: Option<Key>,
    /// Untranslated diagnostic text (subprocess output, third-party data).
    raw: Option<String>,
    args: Vec<(String, Arg)>,
}

/// A placeholder value: literal text, or another catalog entry that is
/// translated together with the message.
#[derive(Debug, Clone)]
enum Arg {
    Text(String),
    Key(Key),
}

impl Message {
    pub fn new(key: Key) -> Self {
        Self {
            key: Some(key),
            raw: None,
            args: Vec::new(),
        }
    }

    /// Text that is already final (diagnostics, captured stderr).
    pub fn raw(value: impl Into<String>) -> Self {
        Self {
            key: None,
            raw: Some(value.into()),
            args: Vec::new(),
        }
    }

    pub fn arg(mut self, name: &str, value: impl fmt::Display) -> Self {
        self.args
            .push((name.to_string(), Arg::Text(value.to_string())));
        self
    }

    /// A placeholder whose value is itself a catalog entry (e.g. the
    /// "Proxy" / "Hugging Face mirror" label in a URL error).
    pub fn arg_key(mut self, name: &str, key: Key) -> Self {
        self.args.push((name.to_string(), Arg::Key(key)));
        self
    }

    pub fn key(&self) -> Option<Key> {
        self.key
    }

    pub fn render(&self, lang: Lang) -> String {
        if let Some(raw) = &self.raw {
            return raw.clone();
        }
        let mut out = text(lang, self.key.expect("message has a key")).to_string();
        for (name, value) in &self.args {
            let rendered = match value {
                Arg::Text(text) => text.clone(),
                Arg::Key(key) => text(lang, *key).to_string(),
            };
            out = out.replace(&format!("{{{name}}}"), &rendered);
        }
        out
    }
}

impl fmt::Display for Message {
    /// Internal/log rendering: English.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render(Lang::En))
    }
}

/// Parse a script progress marker's `key arg=value …` payload.
pub fn message_from_marker(payload: &str) -> Option<Message> {
    let mut parts = payload.split_whitespace();
    let key = Key::from_id(parts.next()?)?;
    let mut message = Message::new(key);
    for part in parts {
        let (name, value) = part.split_once('=')?;
        // Scripts percent-encode spaces so the payload stays one token.
        message = message.arg(name, value.replace("%20", " "));
    }
    Some(message)
}

impl From<Key> for Message {
    fn from(key: Key) -> Self {
        Message::new(key)
    }
}

impl From<&str> for Message {
    fn from(value: &str) -> Self {
        Message::raw(value)
    }
}

impl From<String> for Message {
    fn from(value: String) -> Self {
        Message::raw(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_arguments_per_language() {
        let msg = Message::new(Key::ReferenceAudioRequired).arg("engine", "ZipVoice");
        assert_eq!(
            msg.render(Lang::En),
            "ZipVoice needs a reference clip (3–10 s of clean speech works best)."
        );
        assert!(msg
            .render(Lang::Zh)
            .starts_with("ZipVoice 需要一段参考音频"));
    }

    #[test]
    fn parses_script_markers() {
        let msg = message_from_marker("stage_download_file name=model.onnx size=12.3MB")
            .expect("known key");
        assert_eq!(msg.key(), Some(Key::StageDownloadFile));
        assert_eq!(msg.render(Lang::En), "Downloading model.onnx (12.3MB)");
        assert!(message_from_marker("not_a_key x=1").is_none());
    }

    #[test]
    fn accept_language() {
        assert_eq!(Lang::from_accept_language(Some("zh-CN,zh;q=0.9")), Lang::Zh);
        assert_eq!(Lang::from_accept_language(Some("en-GB")), Lang::En);
        assert_eq!(Lang::from_accept_language(None), Lang::En);
    }
}
