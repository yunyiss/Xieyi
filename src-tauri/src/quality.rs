//! 翻译质量自检：轻量、可解释的规则。
//!
//! 定位不是"打分"，而是**抓粗糙的失败**：截断、空输出、结构被吃掉、术语没落地。
//! 本地小模型翻得不如云端，但这类失败是可检测、可纠正的——发现后带一条
//! 针对性的提示重试一次，通常就能修好。
//!
//! 所有函数都是纯函数，方便单测；不做网络、不碰文件。

/// 术语对。用引用是为了让调用方直接映射 `GlossaryEntry`，不必克隆。
pub struct Term<'a> {
    pub from: &'a str,
    pub to: &'a str,
}

/// 检出的一处问题。带具体数字/词表，方便拼出可解释的重试提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issue {
    /// 没有任何实义字符（空白或纯标点）
    Empty,
    /// 译文相对原文过短，基本是截断
    TooShort { source: usize, target: usize },
    /// 译文相对原文过长，基本是复读或夹带解释
    TooLong { source: usize, target: usize },
    /// 原文有的结构标记在译文里丢了
    Structure(Vec<&'static str>),
    /// 术语表里的原词出现在原文，但指定译法没出现在译文
    Glossary(Vec<String>),
}

impl Issue {
    /// 给用户看的一句话说明。
    pub fn describe(&self) -> String {
        match self {
            Issue::Empty => "输出为空".into(),
            Issue::TooShort { source, target } => {
                format!("疑似截断（原文 {source} 字，译文仅 {target} 字）")
            }
            Issue::TooLong { source, target } => {
                format!("疑似啰嗦或复读（原文 {source} 字，译文 {target} 字）")
            }
            Issue::Structure(kinds) => format!("结构丢失（{}）", kinds.join("、")),
            Issue::Glossary(terms) => format!("术语未命中（{}）", terms.join("、")),
        }
    }
}

/// 一次质检的结果。
#[derive(Debug, Clone)]
pub struct Report {
    pub issues: Vec<Issue>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn describe(&self) -> String {
        self.issues
            .iter()
            .map(Issue::describe)
            .collect::<Vec<_>>()
            .join("；")
    }

    /// 给重试用的英文纠正提示，追加到 system prompt 之后。
    /// 没有问题时不返回内容。
    pub fn retry_hint(&self) -> Option<String> {
        if self.issues.is_empty() {
            return None;
        }

        let mut lines: Vec<String> = Vec::new();

        for issue in &self.issues {
            match issue {
                Issue::Empty => lines.push(
                    "Your previous answer was empty. You must output the translated text."
                        .into(),
                ),
                Issue::TooShort { .. } => lines.push(
                    "Your previous answer was far shorter than the source and looks truncated. \
                     Translate every sentence completely."
                        .into(),
                ),
                Issue::TooLong { .. } => lines.push(
                    "Your previous answer was far longer than the source. Output only the \
                     translation, with no repetition, notes or explanations."
                        .into(),
                ),
                Issue::Structure(kinds) => {
                    let mut kept: Vec<&str> = Vec::new();
                    for kind in kinds {
                        match *kind {
                            "列表" => kept.push("keep every list marker (`-`, `*`, `1.`) at the line start"),
                            "代码块" => kept.push("keep the ``` code fences"),
                            "时间轴" => kept.push("keep the subtitle timestamps such as `00:00:01,000 --> 00:00:03,500`"),
                            _ => {}
                        }
                    }
                    lines.push(format!(
                        "The source has extra structure that your answer lost: {}.",
                        kept.join("; ")
                    ));
                }
                Issue::Glossary(terms) => lines.push(format!(
                    "These term mappings were ignored, use them exactly: {}.",
                    terms
                        .iter()
                        .map(|t| format!("`{t}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }

        lines.push("Output the corrected translation only.".into());
        Some(lines.join(" "))
    }
}

/// 译文短于原文这个比例就判为截断。
///
/// 放得很松是有意的：英文→中文大约剩一半字数，中文→英文大约翻两倍，
/// 正常波动不该被误判。15% 只有"整段没翻出来"才会触发。
const MIN_RATIO: f64 = 0.15;

/// 译文长于原文这个倍数就判为啰嗦/复读。
///
/// 同样取宽松值：中文→英文 4 倍上下是正常的，5 倍以上才像复读。
const MAX_RATIO: f64 = 5.0;

/// 实义字符：字母、数字、CJK。标点和空白不算，用来区分"空输出"和"只有标点"。
fn meaningful_count(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

fn non_space_count(text: &str) -> usize {
    text.chars().filter(|c| !c.is_whitespace()).count()
}

/// 结构标记：原文有、译文一份都没有，说明模型把结构"意译"掉了。
fn missing_structures(source: &str, target: &str) -> Vec<&'static str> {
    let mut missing = Vec::new();

    if list_marks(source) > 0 && list_marks(target) == 0 {
        missing.push("列表");
    }
    if source.matches("```").count() > 0 && target.matches("```").count() == 0 {
        missing.push("代码块");
    }
    if has_timestamp(source) && !has_timestamp(target) {
        missing.push("时间轴");
    }

    missing
}

/// 行首的列表标记数（`- ` / `* ` / `+ ` / `1. `）。
fn list_marks(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let t = line.trim_start();
            t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") || {
                let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
                !digits.is_empty()
                    && digits.len() <= 3
                    && t[digits.len()..].starts_with(". ")
            }
        })
        .count()
}

/// 是否含 srt 风格时间轴（`-->`），只认带冒号的行，避免把箭头符号误判成结构。
fn has_timestamp(text: &str) -> bool {
    text.lines()
        .any(|line| line.contains("-->") && line.contains(':'))
}

/// 术语未命中：原文出现原词、译文却没出现指定译法。
///
/// 只做子串包含判断，会有误判（例如译法只是原词的词形变化），
/// 但代价只是"多一次重试"，不会污染结果，所以宁可宽松。
fn missed_terms(source: &str, target: &str, terms: &[Term]) -> Vec<String> {
    terms
        .iter()
        .filter(|t| !t.from.trim().is_empty() && !t.to.trim().is_empty())
        .filter(|t| source.contains(t.from))
        .filter(|t| !target.contains(t.to))
        .map(|t| format!("{} => {}", t.from, t.to))
        .collect()
}

/// 对一个片段的译文做全套检查。
pub fn check(source: &str, target: &str, terms: &[Term]) -> Report {
    let mut issues = Vec::new();

    let trimmed = target.trim();

    if trimmed.is_empty() || meaningful_count(trimmed) == 0 {
        // 空输出连长度都不用比了，直接定性
        return Report {
            issues: vec![Issue::Empty],
        };
    }

    let source_chars = non_space_count(source);
    let target_chars = non_space_count(trimmed);
    let ratio = target_chars as f64 / source_chars.max(1) as f64;

    if ratio < MIN_RATIO {
        issues.push(Issue::TooShort {
            source: source_chars,
            target: target_chars,
        });
    } else if ratio > MAX_RATIO {
        issues.push(Issue::TooLong {
            source: source_chars,
            target: target_chars,
        });
    }

    let missing = missing_structures(source, trimmed);
    if !missing.is_empty() {
        issues.push(Issue::Structure(missing));
    }

    let missed = missed_terms(source, trimmed, terms);
    if !missed.is_empty() {
        issues.push(Issue::Glossary(missed));
    }

    Report { issues }
}

/// 两次质检谁更好：问题条数少者胜。用于决定重试结果要不要覆盖首次结果。
pub fn better_than(candidate: &str, candidate_report: &Report, first: &str, first_report: &Report) -> bool {
    match candidate_report.issues.len().cmp(&first_report.issues.len()) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Greater => false,
        // 问题一样多时，别用更短的那个（截断风险）
        std::cmp::Ordering::Equal => non_space_count(candidate) > non_space_count(first),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(pairs: &[(&'static str, &'static str)]) -> Vec<Term<'static>> {
        pairs
            .iter()
            .map(|(from, to)| Term { from, to })
            .collect()
    }

    #[test]
    fn empty_or_punctuation_only_is_flagged() {
        assert!(!check("hello", "", &[]).is_ok());
        assert!(!check("hello", "   ", &[]).is_ok());
        assert!(!check("hello", "...", &[]).is_ok());
        // 空输出只报一条，不叠"过短"
        assert_eq!(check("hello", "", &[]).issues.len(), 1);
    }

    #[test]
    fn normal_matches_are_clean() {
        // 英文→中文，字数约一半
        assert!(check(
            "Hello world, this is a test sentence.",
            "你好世界，这是一句测试文字。",
            &[]
        )
        .is_ok());

        // 中文→英文，字数翻倍也是正常的
        assert!(check(
            "这是一句用来测试的中文句子，长度适中。",
            "This is a Chinese sentence used for testing, and its length is moderate.",
            &[]
        )
        .is_ok());
    }

    #[test]
    fn truncated_output_is_flagged() {
        let source = "这是一段比较长的说明文字，用来模拟被截断的情况。它包含多个句子，模型只翻了开头就停了。";
        let target = "这";
        let report = check(source, target, &[]);
        assert!(matches!(report.issues[0], Issue::TooShort { .. }));
    }

    #[test]
    fn verbose_output_is_flagged() {
        let source = "Hello.";
        let target = "你好。".repeat(40);
        let report = check(source, &target, &[]);
        assert!(report.issues.iter().any(|i| matches!(i, Issue::TooLong { .. })));
    }

    #[test]
    fn lost_list_structure_is_flagged() {
        let source = "- apple\n- banana\n- cherry";
        let target = "苹果香蕉樱桃";
        let report = check(source, target, &[]);
        assert!(report
            .issues
            .iter()
            .any(|i| matches!(i, Issue::Structure(_))));

        // 保留列表标记就不该报
        assert!(check(source, "- 苹果\n- 香蕉\n- 樱桃", &[]).is_ok());
    }

    #[test]
    fn lost_code_fence_is_flagged() {
        let source = "用法：\n```\nrun --now\n```";
        let target = "用法：运行 --now";
        assert!(check(source, target, &[])
            .issues
            .iter()
            .any(|i| matches!(i, Issue::Structure(_))));
    }

    #[test]
    fn lost_subtitle_timestamp_is_flagged() {
        let source = "1\n00:00:01,000 --> 00:00:03,500\nHello";
        let target = "1\n你好";
        let report = check(source, target, &[]);
        assert!(report
            .issues
            .iter()
            .any(|i| matches!(i, Issue::Structure(_))));
    }

    #[test]
    fn arrow_without_colon_is_not_a_timestamp() {
        // 普通文本里的箭头不该被当成 srt
        assert!(check("a --> b", "甲 --> 乙", &[]).is_ok());
    }

    #[test]
    fn missed_glossary_term_is_flagged() {
        let report = check(
            "The server is down.",
            "服务宕机了。",
            &terms(&[("server", "服务器")]),
        );
        assert!(matches!(
            report.issues.last(),
            Some(Issue::Glossary(_))
        ));

        // 用上了指定译法就通过
        assert!(check("The server is down.", "服务器宕机了。", &terms(&[("server", "服务器")]))
            .is_ok());
        // 原词没出现在原文里，不检查
        assert!(check("Hello.", "你好。", &terms(&[("server", "服务器")])).is_ok());
    }

    #[test]
    fn hint_mentions_the_actual_problem() {
        // 译文既丢了列表，也没用上术语表指定的"苹果"
        let report = check(
            "- apple\n- banana",
            "红果和香蕉",
            &terms(&[("apple", "苹果")]),
        );
        let hint = report.retry_hint().expect("有问题就该给出提示");
        assert!(hint.contains("list marker"), "提示应点名列表：{hint}");
        assert!(hint.contains("apple => 苹果"), "提示应带上具体术语：{hint}");

        assert!(check("hello", "你好", &[]).retry_hint().is_none());
    }

    #[test]
    fn descriptions_are_specific_enough_for_the_ui() {
        let report = check("hello world", "h", &[]);
        let text = report.describe();
        assert!(text.contains("截断"), "应说明是截断：{text}");
    }

    #[test]
    fn better_than_prefers_fewer_issues_then_longer_text() {
        let bad = check("hello world foo bar", "x", &[]);
        let good = check("hello world foo bar", "你好世界，富吧。", &[]);
        assert!(better_than("你好世界，富吧。", &good, "x", &bad));

        // 问题同样多时，长的那个更可能是完整输出
        let a = check("hello", "a", &[]);
        let b = check("hello", "ab", &[]);
        assert_eq!(a.issues.len(), b.issues.len());
        assert!(better_than("ab", &b, "a", &a));
        assert!(!better_than("a", &a, "ab", &b));
    }
}
