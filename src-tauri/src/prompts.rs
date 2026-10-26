//! 翻译提示词预设。
//!
//! 对齐 STranslate 的 `Prompts` 思路：把"怎么翻"从代码里拆成可选风格，
//! 用户按内容类型选一套。风格只影响 system prompt —— 请求形状、输出上限、
//! 思考开关都不变。
//!
//! 缓存键会带上风格：同一段原文换风格后会重新翻译，不会命中旧结果。

use crate::quality;
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

/// 在基础提示后面追加质检给出的纠正要求，用于重试。
pub fn system_prompt_with_hint(
    style: Style,
    source: &str,
    target: &str,
    glossary: &[GlossaryEntry],
    report: &quality::Report,
) -> String {
    let mut prompt = system_prompt(style, source, target, glossary);
    if let Some(hint) = report.retry_hint() {
        prompt.push_str("\n\nYour previous attempt had problems. ");
        prompt.push_str(&hint);
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let base = system_prompt(Style::Formal, "en", "zh", &glossary());
        let hinted = system_prompt_with_hint(Style::Formal, "en", "zh", &glossary(), &report);
        assert!(hinted.starts_with(&base), "重试提示应建立在基础提示之上");
        assert!(hinted.contains("Your previous attempt had problems."));
        assert!(hinted.contains("apple => 苹果"), "应带上具体术语：{hinted}");
    }
}
