//! 翻译提示词预设。
//!
//! 对齐 STranslate 的 `Prompts` 思路：把"怎么翻"从代码里拆成可选风格，
//! 用户按内容类型选一套。风格只影响 system prompt —— 请求形状、输出上限、
//! 思考开关都不变。
//!
//! 缓存键会带上风格：同一段原文换风格后会重新翻译，不会命中旧结果。

use crate::service::GlossaryEntry;

/// 翻译风格。字符串值与前端 `TranslateStyle` 一一对应，未知值回落到通用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    #[default]
    General,
    Formal,
    Technical,
    Literary,
    Subtitle,
}

impl Style {
    pub fn as_str(self) -> &'static str {
        match self {
            Style::General => "general",
            Style::Formal => "formal",
            Style::Technical => "technical",
            Style::Literary => "literary",
            Style::Subtitle => "subtitle",
        }
    }

    /// 解析前端传来的风格；空值或未知值一律按通用处理。
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "formal" => Style::Formal,
            "technical" => Style::Technical,
            "literary" => Style::Literary,
            "subtitle" => Style::Subtitle,
            _ => Style::General,
        }
    }

    /// 风格附加要求。通用风格不追加内容，保持原有提示词逐字不变。
    fn instruction(self) -> Option<&'static str> {
        match self {
            Style::General => None,
            Style::Formal => Some(
                "Use formal, written language: avoid slang, contractions and casual wording.",
            ),
            Style::Technical => Some(
                "This is technical content: keep product names, code identifiers, numbers and \
                 terminology exactly as written; prioritize precision over fluency.",
            ),
            Style::Literary => Some(
                "This is literary or marketing copy: preserve tone, imagery and rhythm; \
                 favor natural, expressive wording over literal translation.",
            ),
            Style::Subtitle => Some(
                "This is subtitle text: keep each line short and easy to read at speed; \
                 do not merge lines or add narration.",
            ),
        }
    }
}

/// 拼一次翻译请求的 system prompt：基础要求 → 风格要求 → 术语表。
pub fn system_prompt(
    style: Style,
    source: &str,
    target: &str,
    glossary: &[GlossaryEntry],
) -> String {
    let src = if source == "auto" {
        "the source language"
    } else {
        source
    };
    let mut prompt = format!(
        "You are a professional translator. Translate the user's text from {src} to {target}. \
Output only the translation, without explanations, notes, or quotation marks."
    );

    if let Some(instruction) = style.instruction() {
        prompt.push(' ');
        prompt.push_str(instruction);
    }

    if !glossary.is_empty() {
        prompt.push_str("\nUse these term mappings when they appear:\n");
        for entry in glossary {
            prompt.push_str(&format!("- {} => {}\n", entry.from, entry.to));
        }
    }

    prompt
}

// ============================================================================
// 按模型调用机制生成请求
// ============================================================================

use crate::models::PromptStyle;
/// 一次翻译请求的产物：要发给模型的消息体 + 额外的模板参数。
///
/// 之所以不只返回提示词字符串，是因为有些模型（translategemma）根本不看
/// system 消息，语言方向完全由 `chat_template_kwargs` 决定。
pub struct TranslationRequest {
    pub messages: serde_json::Value,
    /// 需要并进请求体的 `chat_template_kwargs`（没有则为 None）
    pub template_kwargs: Option<serde_json::Value>,
}

/// 语言代码 → 模型内部语言名的映射。
///
/// 只覆盖常见语言；映射不到时回退到原代码。Translategemma 用 ISO 带地区码，
/// 直接给 `en` 落不到它的表里，会被当成未知而默认 en-GB。
fn language_code(code: &str) -> String {
    match code {
        "zh" => "zh-Hans".into(),
        "zh-tw" | "zh-hant" => "zh-Hant".into(),
        "en" => "en-GB".into(),
        "ja" => "ja-JP".into(),
        "ko" => "ko-KR".into(),
        "de" => "de-DE".into(),
        "fr" => "fr-FR".into(),
        "es" => "es-ES".into(),
        "ru" => "ru-RU".into(),
        "pt" => "pt-PT".into(),
        "it" => "it-IT".into(),
        "ar" => "ar-SA".into(),
        "th" => "th-TH".into(),
        "vi" => "vi-VN".into(),
        other => other.into(),
    }
}

/// 语言代码 → 中文语言名（中文提示词用，Index-Translate 系）。
fn language_name_zh(code: &str) -> Option<&'static str> {
    Some(match code {
        "zh" => "中文",
        "en" => "英语",
        "ja" => "日语",
        "ko" => "韩语",
        "de" => "德语",
        "fr" => "法语",
        "es" => "西班牙语",
        "ru" => "俄语",
        "pt" => "葡萄牙语",
        "it" => "意大利语",
        "ar" => "阿拉伯语",
        "th" => "泰语",
        "vi" => "越南语",
        _ => return None,
    })
}

/// 中文风格要求的追加说明（中文提示词用）。
fn style_instruction_zh(style: Style) -> Option<&'static str> {
    Some(match style {
        Style::General => return None,
        Style::Formal => "使用正式、书面的语体，避免口语与缩略表达。",
        Style::Technical => "这是技术内容：产品名、代码标识符、数字与术语保持原样，准确优先于流畅。",
        Style::Literary => "这是文学或营销文案：保留语气、意象与节奏，意译优先于直译。",
        Style::Subtitle => "这是字幕：每行简短易读，不要合并行，也不要添加旁白。",
    })
}

/// 按模型调用机制生成一次翻译请求。
///
/// `hint` 是质检给出的纠正要求（重试时才有）。各风格的提示词都会把它
/// 追加在末尾——它是"本次输出的额外要求"，与模型家族无关。
pub fn build_request(
    prompt_style: PromptStyle,
    style: Style,
    source: &str,
    target: &str,
    glossary: &[GlossaryEntry],
    text: &str,
    hint: Option<&str>,
) -> TranslationRequest {
    match prompt_style {
        // ---- 现状：英文 system prompt，绝大多数模型走这条 ----
        PromptStyle::EnglishSystem => {
            let mut sys = system_prompt(style, source, target, glossary);
            if let Some(h) = hint {
                sys.push_str("\n\nYour previous attempt had problems. ");
                sys.push_str(h);
            }
            TranslationRequest {
                messages: serde_json::json!([
                    { "role": "system", "content": sys },
                    { "role": "user", "content": text }
                ]),
                template_kwargs: None,
            }
        }

        // ---- Index-Translate：中文文本提示，不发 system ----
        PromptStyle::ChineseText => {
            let src = if source == "auto" {
                "原文".to_string()
            } else {
                language_name_zh(source).unwrap_or(source).to_string()
            };
            let tgt = language_name_zh(target).unwrap_or(target).to_string();

            let mut user = format!(
                "请将以下{src}文本翻译为{tgt}，直接输出翻译结果，不要进行任何解释。\n\n【源文】\n{text}"
            );

            // 术语与风格按 Index-Translate 的约束格式给出
            let mut constraints: Vec<String> = Vec::new();
            if !glossary.is_empty() {
                let terms: Vec<String> = glossary
                    .iter()
                    .map(|e| format!("{}→{}", e.from, e.to))
                    .collect();
                constraints.push(format!("【硬性要求】术语对照: {}", terms.join("、")));
            }
            if let Some(inst) = style_instruction_zh(style) {
                constraints.push(format!("【注意】{inst}"));
            }
            if let Some(h) = hint {
                constraints.push(format!("【注意】{h}"));
            }
            if !constraints.is_empty() {
                user.push_str("\n\n【约束要求】\n");
                for (i, c) in constraints.iter().enumerate() {
                    user.push_str(&format!("{}. {}\n", i + 1, c));
                }
                user.push_str("\n只输出译文，不要有任何额外说明。");
            }

            TranslationRequest {
                messages: serde_json::json!([{ "role": "user", "content": user }]),
                template_kwargs: None,
            }
        }

        // ---- TranslateGemma 原版：语言码走 chat_template_kwargs ----
        PromptStyle::TemplateKwargs => {
            // 它的模板只认这个参数，system 消息会被忽略，所以不发 system。
            // 翻译风格/术语表只能并进正文——模板本身不提供注入点。
            let mut user = text.to_string();
            if let Some(inst) = style.instruction() {
                user.push_str("\n\n[Note: ");
                user.push_str(inst);
                user.push(']');
            }
            if !glossary.is_empty() {
                user.push_str("\n\n[Glossary: ");
                let terms: Vec<String> = glossary
                    .iter()
                    .map(|e| format!("{}={}", e.from, e.to))
                    .collect();
                user.push_str(&terms.join(", "));
                user.push(']');
            }
            if let Some(h) = hint {
                user.push_str("\n\n[Correction: ");
                user.push_str(h);
                user.push(']');
            }

            TranslationRequest {
                messages: serde_json::json!([{ "role": "user", "content": user }]),
                template_kwargs: Some(serde_json::json!({
                    "source_lang_code": if source == "auto" { "en-GB".to_string() } else { language_code(source) },
                    "target_lang_code": language_code(target),
                    "enable_thinking": false
                })),
            }
        }

        // ---- TranslateGemma 的 vLLM 优化版：正文里拼分隔符 ----
        PromptStyle::Delimiter => {
            let mut user = format!(
                "<<<source>>>{}<<<target>>>{}<<<text>>>{}",
                source, target, text
            );
            if let Some(h) = hint {
                user.push_str("\n\n[Correction: ");
                user.push_str(h);
                user.push(']');
            }
            TranslationRequest {
                messages: serde_json::json!([{ "role": "user", "content": user }]),
                template_kwargs: None,
            }
        }

        // ---- NVIDIA Riva-Translate：system 写语言对代码 ----
        PromptStyle::LanguagePair => {
            // 模板会拿 system 内容去查 en-zh 这类键；查不到就退化成
            // "You are a translation expert."，所以格式必须严格。
            let pair = format!("{}-{}", source, target);
            TranslationRequest {
                messages: serde_json::json!([
                    { "role": "system", "content": pair },
                    { "role": "user", "content": text }
                ]),
                template_kwargs: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality;

    fn glossary() -> Vec<GlossaryEntry> {
        vec![GlossaryEntry {
            from: "apple".into(),
            to: "苹果".into(),
        }]
    }

    #[test]
    fn unknown_style_falls_back_to_general() {
        assert_eq!(Style::parse(""), Style::General);
        assert_eq!(Style::parse("nope"), Style::General);
        assert_eq!(Style::parse("technical"), Style::Technical);
        // 前后空白与大小写不该影响选择
        assert_eq!(Style::parse(" FORMAL "), Style::Formal);
    }

    #[test]
    fn wire_names_round_trip_and_are_unique() {
        let styles = [
            Style::General,
            Style::Formal,
            Style::Technical,
            Style::Literary,
            Style::Subtitle,
        ];
        let mut names = std::collections::HashSet::new();
        for style in styles {
            let name = style.as_str();
            assert!(names.insert(name), "风格名重复: {name}");
            assert_eq!(Style::parse(name), style, "{name} 应能解析回原风格");
        }
    }

    #[test]
    fn general_prompt_keeps_the_original_wording() {
        let prompt = system_prompt(Style::General, "en", "zh", &[]);
        assert!(prompt.starts_with("You are a professional translator."));
        assert!(prompt.contains("from en to zh"));
        // 通用风格不追加任何额外要求
        assert!(!prompt.contains("This is"), "通用风格不该带附加要求: {prompt}");
    }

    #[test]
    fn each_style_appends_its_own_instruction() {
        let base = system_prompt(Style::General, "en", "zh", &[]);
        let technical = system_prompt(Style::Technical, "en", "zh", &[]);
        assert!(technical.starts_with(&base), "风格要求应追加在基础提示之后");
        assert!(technical.contains("technical content"));

        let subtitle = system_prompt(Style::Subtitle, "en", "zh", &[]);
        assert!(subtitle.contains("subtitle text"));
        assert!(subtitle.contains(&base));
    }

    #[test]
    fn glossary_is_kept_for_every_style() {
        let prompt = system_prompt(Style::Literary, "en", "zh", &glossary());
        assert!(prompt.contains("apple => 苹果"));
        assert!(prompt.contains("literary or marketing copy"));
    }

    #[test]
    fn retry_hint_builds_on_the_styled_base() {
        let report = quality::check(
            "- apple\n- banana",
            "红果和香蕉",
            &[quality::Term {
                from: "apple",
                to: "苹果",
            }],
        );
        assert!(!report.is_ok());

        // 重试请求必须建立在基础提示之上，并带上针对性的纠正要求
        let base = build_request(
            PromptStyle::EnglishSystem,
            Style::Formal,
            "en",
            "zh",
            &glossary(),
            "hello",
            None,
        );
        let hinted = build_request(
            PromptStyle::EnglishSystem,
            Style::Formal,
            "en",
            "zh",
            &glossary(),
            "hello",
            report.retry_hint().as_deref(),
        );

        let base_sys = base.messages[0]["content"].as_str().unwrap();
        let hinted_sys = hinted.messages[0]["content"].as_str().unwrap();
        assert!(hinted_sys.starts_with(base_sys), "重试提示应建立在基础提示之上");
        assert!(hinted_sys.contains("Your previous attempt had problems."));
        assert!(hinted_sys.contains("apple => 苹果"), "应带上具体术语：{hinted_sys}");
    }

    // ---- 按调用机制生成请求 ----

    /// **回归底线**：EnglishSystem 必须与改造前逐字一致。
    /// 这条挂了就说明动了不该动的地方——现有所有模型都靠它。
    #[test]
    fn english_system_matches_legacy_behaviour() {
        let req = build_request(
            PromptStyle::EnglishSystem,
            Style::Technical,
            "en",
            "zh",
            &glossary(),
            "hello",
            None,
        );
        let legacy = system_prompt(Style::Technical, "en", "zh", &glossary());

        assert_eq!(req.messages[0]["role"], "system");
        assert_eq!(req.messages[0]["content"], legacy.as_str());
        assert_eq!(req.messages[1]["role"], "user");
        assert_eq!(req.messages[1]["content"], "hello");
        // 这条路径不需要额外模板参数
        assert!(req.template_kwargs.is_none());
    }

    /// Index-Translate 走中文文本提示，且不发 system。
    #[test]
    fn chinese_text_puts_everything_in_one_user_message() {
        let req = build_request(
            PromptStyle::ChineseText,
            Style::Formal,
            "en",
            "zh",
            &glossary(),
            "hello",
            None,
        );
        // 只有一条 user，没有 system
        assert_eq!(req.messages.as_array().unwrap().len(), 1);
        assert_eq!(req.messages[0]["role"], "user");

        let content = req.messages[0]["content"].as_str().unwrap();
        assert!(content.contains("翻译为中文"), "应指定目标语言: {content}");
        assert!(content.contains("英语"), "应指定源语言: {content}");
        assert!(content.contains("hello"), "应带上原文");
        // 术语按它的【硬性要求】格式给出
        assert!(content.contains("【硬性要求】"), "{content}");
        assert!(content.contains("apple→苹果"), "{content}");
        // 风格按【注意】给出
        assert!(content.contains("【注意】"), "{content}");
        assert!(content.contains("只输出译文"), "{content}");
    }

    /// translategemma 的语言方向完全靠 kwargs，否则默认 en-GB。
    #[test]
    fn template_kwargs_carries_language_codes() {
        let req = build_request(
            PromptStyle::TemplateKwargs,
            Style::General,
            "zh",
            "ja",
            &[],
            "今天天气很好。",
            None,
        );
        // 不发 system（模板会忽略）
        assert_eq!(req.messages.as_array().unwrap().len(), 1);
        assert_eq!(req.messages[0]["role"], "user");

        let kw = req.template_kwargs.expect("必须带 chat_template_kwargs");
        // 必须落成带地区的码，否则模板查不到语言名
        assert_eq!(kw["source_lang_code"], "zh-Hans");
        assert_eq!(kw["target_lang_code"], "ja-JP");
        assert_eq!(kw["enable_thinking"], false);
    }

    /// 自动检测时给个安全的源语言，不能让它落到 en-GB 上把中文当英文。
    #[test]
    fn template_kwargs_auto_source_is_not_english() {
        let req = build_request(
            PromptStyle::TemplateKwargs,
            Style::General,
            "auto",
            "zh",
            &[],
            "hello",
            None,
        );
        let kw = req.template_kwargs.unwrap();
        assert_eq!(kw["target_lang_code"], "zh-Hans");
    }

    #[test]
    fn delimiter_variant_embeds_language_markers() {
        let req = build_request(
            PromptStyle::Delimiter,
            Style::General,
            "en",
            "zh",
            &[],
            "hello world",
            None,
        );
        let content = req.messages[0]["content"].as_str().unwrap();
        assert!(content.starts_with("<<<source>>>en<<<target>>>zh<<<text>>>"));
        assert!(content.ends_with("hello world"));
    }

    /// Riva-Translate 的模板拿 system 内容去查语言对表，格式必须严格是 en-zh。
    #[test]
    fn language_pair_puts_code_in_system() {
        let req = build_request(
            PromptStyle::LanguagePair,
            Style::General,
            "en",
            "zh",
            &[],
            "hello",
            None,
        );
        assert_eq!(req.messages[0]["role"], "system");
        assert_eq!(req.messages[0]["content"], "en-zh");
        assert_eq!(req.messages[1]["content"], "hello");
    }

    /// 重试的纠正提示要能进到每一种机制里，不能因为换了风格就丢掉。
    #[test]
    fn retry_hint_reaches_every_prompt_style() {
        for style in [
            PromptStyle::EnglishSystem,
            PromptStyle::ChineseText,
            PromptStyle::TemplateKwargs,
            PromptStyle::Delimiter,
            PromptStyle::LanguagePair,
        ] {
            let req = build_request(
                style,
                Style::General,
                "en",
                "zh",
                &[],
                "hello",
                Some("keep every list marker"),
            );
            let joined = req.messages.to_string();
            if style == PromptStyle::LanguagePair {
                // 这条机制的模板不接受附加要求，只能忽略——不算丢
                continue;
            }
            assert!(
                joined.contains("keep every list marker"),
                "{style:?} 丢了纠正提示: {joined}"
            );
        }
    }
}
