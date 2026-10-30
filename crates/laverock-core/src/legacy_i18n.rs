//! Fill in English text for manifests written before the bilingual pass.
//!
//! Engine manifests used to carry Chinese-only labels ("语速", "小贝 ·
//! 普通话女声 (zf_xiaobei)"). The UI shows a `Localized::Text` verbatim, so
//! those installs render Chinese text in an English interface. Rather than
//! ask the user to re-download several gigabytes, the loader upgrades the
//! labels in memory:
//!
//! 1. an `"中文 / English"` label splits at the slash;
//! 2. a parameter the built-in set also declares takes the missing side
//!    from it (matched by name, options by value);
//! 3. a phrase we authored ourselves is looked up in the table below.
//!
//! Anything still untranslated stays as it was; nothing is written back.

use crate::manifest::{EngineKind, Localized, Manifest, ParamSpec};

/// Labels and hints our own installers wrote before the bilingual pass.
const PHRASES: &[(&str, &str)] = &[
    ("音色", "Voice"),
    (
        "Kokoro 只提供预置音色，不支持用自己的音频克隆。",
        "Kokoro offers preset voices only; it cannot clone your own audio.",
    ),
    ("语速", "Speed"),
    ("越大语速越快，不改变音色", "Higher is faster; the voice itself is unchanged."),
    (
        "九个内置预置音色，无需参考音频。",
        "Nine built-in preset voices; no reference audio needed.",
    ),
    (
        "可选，用自然语言调整语气，如「用兴奋的语气」。",
        "Optional: adjust the tone in natural language, e.g. \"use an excited tone\".",
    ),
    (
        "必填：用自然语言描述想要的音色，如「一位温柔的年轻女性，语速稍慢」。",
        "Required: describe the voice in natural language, e.g. \"a gentle young woman, a little slower\".",
    ),
];

/// Kokoro's Chinese voice descriptors, e.g. "美式英语女声" -> "American
/// English (female)". The voice name and the `(af_alloy)` id are kept.
const VOICE_TRAITS: &[(&str, &str)] = &[
    ("美式英语女声", "American English (female)"),
    ("美式英语男声", "American English (male)"),
    ("英式英语女声", "British English (female)"),
    ("英式英语男声", "British English (male)"),
    ("普通话女声", "Mandarin (female)"),
    ("普通话男声", "Mandarin (male)"),
    ("日语女声", "Japanese (female)"),
    ("日语男声", "Japanese (male)"),
    ("西班牙语女声", "Spanish (female)"),
    ("西班牙语男声", "Spanish (male)"),
    ("法语女声", "French (female)"),
    ("意大利语女声", "Italian (female)"),
    ("意大利语男声", "Italian (male)"),
    ("印地语女声", "Hindi (female)"),
    ("印地语男声", "Hindi (male)"),
    ("葡萄牙语女声", "Portuguese (female)"),
    ("葡萄牙语男声", "Portuguese (male)"),
];

/// The eight Kokoro Mandarin voices are named in Chinese in the model.
const VOICE_NAMES: &[(&str, &str)] = &[
    ("小贝", "Xiaobei"),
    ("小妮", "Xiaoni"),
    ("晓晓", "Xiaoxiao"),
    ("小伊", "Xiaoyi"),
    ("云健", "Yunjian"),
    ("云希", "Yunxi"),
    ("云夏", "Yunxia"),
    ("云扬", "Yunyang"),
];

/// True when `text` contains a CJK ideograph (the marker of a label written
/// for a Chinese-only UI).
pub fn contains_cjk(text: &str) -> bool {
    text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// Upgrade every label and hint in `params` that lacks an English side.
pub fn localize(kind: EngineKind, params: &mut [ParamSpec]) {
    let builtin = Manifest::default_parameters(kind);
    for param in params.iter_mut() {
        let spec = builtin.iter().find(|b| b.name == param.name);
        param.label = localize_one(&param.label, spec.map(|b| &b.label));
        param.hint = localize_one(&param.hint, spec.map(|b| &b.hint));
        for option in param.options.iter_mut() {
            let fallback = spec
                .and_then(|b| b.options.iter().find(|o| o.value == option.value))
                .map(|o| &o.label);
            option.label = localize_one(&option.label, fallback);
        }
    }
}

/// Whether any label would still show Chinese in an English UI.
pub fn any_untranslated(kind: EngineKind, params: &[ParamSpec]) -> bool {
    let mut params = params.to_vec();
    localize(kind, &mut params);
    // Option labels are exempt: engines label languages in both scripts on
    // purpose ("Chinese 中文"), and `Localized::matches` depends on it.
    params
        .iter()
        .any(|p| contains_cjk(p.label.for_lang("en")) || contains_cjk(p.hint.for_lang("en")))
}

/// Fill the missing side of `value`, preferring `fallback` (the built-in
/// spec's string) over the phrase tables.
fn localize_one(value: &Localized, fallback: Option<&Localized>) -> Localized {
    match value {
        Localized::Translations { en, zh } => {
            let en = en.clone().or_else(|| {
                zh.as_deref()
                    .and_then(phrase)
                    .map(str::to_string)
                    .or_else(|| fallback.and_then(en_of).map(str::to_string))
                    .or_else(|| zh.clone().filter(|z| !contains_cjk(z)))
            });
            let zh = zh
                .clone()
                .or_else(|| fallback.and_then(zh_of).map(str::to_string));
            Localized::Translations { en, zh }
        }
        Localized::Text(text) => {
            if text.is_empty() {
                return value.clone();
            }
            if !contains_cjk(text) {
                // Already English (or blank): keep the text and borrow the
                // Chinese side from the built-in spec when we have one.
                return Localized::Translations {
                    en: Some(text.clone()),
                    zh: fallback.and_then(zh_of).map(str::to_string),
                };
            }
            if let Some((zh, en)) = split_slash(text) {
                return Localized::Translations {
                    en: Some(en),
                    zh: Some(zh),
                };
            }
            if let Some(en) = phrase(text)
                .map(str::to_string)
                .or_else(|| voice_label(text))
                .or_else(|| fallback.and_then(en_of).map(str::to_string))
            {
                return Localized::Translations {
                    en: Some(en),
                    zh: Some(text.clone()),
                };
            }
            value.clone()
        }
    }
}

fn en_of(value: &Localized) -> Option<&str> {
    match value {
        Localized::Translations { en, .. } => en.as_deref().filter(|e| !e.is_empty()),
        Localized::Text(_) => None,
    }
}

fn zh_of(value: &Localized) -> Option<&str> {
    match value {
        Localized::Translations { zh, .. } => zh.as_deref().filter(|z| !z.is_empty()),
        Localized::Text(_) => None,
    }
}

fn phrase(text: &str) -> Option<&'static str> {
    PHRASES
        .iter()
        .find(|(zh, _)| *zh == text)
        .map(|(_, en)| *en)
}

/// "预置音色 / Preset speaker" and the Qwen speaker lines use a slash.
/// The English side is what follows it; when the Chinese side starts with
/// the speaker's name ("Dylan · 北京男声（清透自然）"), keep the name.
fn split_slash(text: &str) -> Option<(String, String)> {
    let (left, right) = text.split_once(" / ")?;
    let right = right.trim();
    if right.is_empty() || contains_cjk(right) {
        return None;
    }
    let zh = left.trim();
    let name = zh.split(" · ").next().unwrap_or("");
    let en = if name.is_empty() || contains_cjk(name) {
        right.to_string()
    } else {
        format!("{name} · {right}")
    };
    Some((zh.to_string(), en))
}

/// "小贝 · 普通话女声 (zf_xiaobei)" -> "Xiaobei · Mandarin (female) (zf_xiaobei)".
fn voice_label(text: &str) -> Option<String> {
    let mut out = text.to_string();
    for (zh, en) in VOICE_NAMES {
        out = out.replace(zh, en);
    }
    let mut changed = out != text;
    for (zh, en) in VOICE_TRAITS {
        if out.contains(zh) {
            out = out.replace(zh, en);
            changed = true;
        }
    }
    changed.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ParamKind;
    use serde_json::json;

    fn text_param(name: &str, label: &str, hint: &str) -> ParamSpec {
        ParamSpec::new(name, label, ParamKind::Float, json!(1.0), hint)
    }

    #[test]
    fn longcat_labels_come_from_the_builtin_set() {
        let mut params = vec![
            text_param(
                "num_steps",
                "采样步数 (NFE)",
                "扩散 ODE 步数；16 已足够，越多越慢",
            ),
            text_param("cfg", "引导强度", "越高越贴近参考音色，过高可能失真"),
        ];
        localize(EngineKind::LongCat, &mut params);
        assert_eq!(params[0].label.for_lang("en"), "Sampling steps (NFE)");
        assert_eq!(params[0].label.for_lang("zh"), "采样步数 (NFE)");
        assert_eq!(
            params[1].hint.for_lang("en"),
            "Higher follows the reference voice more closely; too high may distort."
        );
        assert!(!any_untranslated(EngineKind::LongCat, &params));
    }

    #[test]
    fn kokoro_voice_options_are_derived_from_their_names() {
        let mut params = vec![ParamSpec {
            options: vec![
                crate::manifest::ParamOption {
                    label: Localized::Text("Alloy · 美式英语女声 (af_alloy)".into()),
                    value: "0".into(),
                },
                crate::manifest::ParamOption {
                    label: Localized::Text("小贝 · 普通话女声 (zf_xiaobei)".into()),
                    value: "53".into(),
                },
            ],
            ..text_param("voice", "音色", "")
        }];
        localize(EngineKind::Kokoro, &mut params);
        assert_eq!(params[0].label.for_lang("en"), "Voice");
        assert_eq!(
            params[0].options[0].label.for_lang("en"),
            "Alloy · American English (female) (af_alloy)"
        );
        assert_eq!(
            params[0].options[0].label.for_lang("zh"),
            "Alloy · 美式英语女声 (af_alloy)"
        );
        assert_eq!(
            params[0].options[1].label.for_lang("en"),
            "Xiaobei · Mandarin (female) (zf_xiaobei)"
        );
        assert!(!any_untranslated(EngineKind::Kokoro, &params));
    }

    #[test]
    fn qwen_slash_labels_keep_the_speaker_name() {
        let mut params = vec![
            text_param(
                "speaker",
                "预置音色 / Preset speaker",
                "九个内置预置音色，无需参考音频。",
            ),
            text_param("instruct", "语气指令 / Instruct", ""),
        ];
        localize(EngineKind::Qwen3Tts, &mut params);
        assert_eq!(params[0].label.for_lang("en"), "Preset speaker");
        assert_eq!(params[0].label.for_lang("zh"), "预置音色");
        assert_eq!(
            params[0].hint.for_lang("en"),
            "Nine built-in preset voices; no reference audio needed."
        );
        assert_eq!(params[1].label.for_lang("en"), "Instruct");
    }

    #[test]
    fn slash_rule_keeps_an_ascii_name() {
        let (zh, en) = split_slash("Dylan · 北京男声（清透自然） / clear Beijing male").unwrap();
        assert_eq!(zh, "Dylan · 北京男声（清透自然）");
        assert_eq!(en, "Dylan · clear Beijing male");
    }

    #[test]
    fn bilingual_and_third_party_labels_are_left_alone() {
        let mut params = vec![
            text_param("speed", "Speed", ""),
            ParamSpec::new(
                "speed",
                ("Speed", "语速"),
                ParamKind::Float,
                json!(1.0),
                ("Higher is faster.", "越大越快"),
            ),
            text_param("strength", "我的强度", ""),
        ];
        localize(EngineKind::ZipVoice, &mut params);
        assert_eq!(params[0].label.for_lang("zh"), "语速");
        assert_eq!(params[1].label.for_lang("en"), "Speed");
        // Unknown Chinese text has no translation to offer: kept as-is.
        assert!(matches!(params[2].label, Localized::Text(_)));
        assert!(any_untranslated(EngineKind::ZipVoice, &params));
    }
}
