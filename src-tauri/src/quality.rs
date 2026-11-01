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

/// 纠正提示该用哪种语言写。
///
/// 与 `models::PromptStyle` 对应：走 `ChineseOfficial` 的模型（Hy-MT2 系）
/// 用中文提示，其余用英文。原因见 [`Report::retry_hint`] 的注释。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintLang {
    English,
    Chinese,
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
    /// 译文与原文一字不差——模型根本没翻，把原文原样吐回来了。
    ///
    /// 这条抓的是最皮但也最容易漏的一种失败：长度比例正常（1.0）、
    /// 没有结构问题、没有术语要求，于是所有既有规则都放行。
    /// 实测 Hy-MT2-1.8B 收到 `hello`、`Apple` 时会这样；
    /// 更大的模型偶尔也会在"看起来不用翻"的短词上偷懒。
    ///
    /// 判定不能只看"相等"——`API`、`OK`、`PDF` 本来就该原样保留。
    /// 具体规则见 [`should_ignore_untranslated`]。
    Untranslated,
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
            Issue::Untranslated => "译文与原文完全一致，未发生实际翻译".into(),
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

    /// 给重试用的纠正提示，追加到 system prompt 之后。
    /// 没有问题时不返回内容。
    ///
    /// `hint_lang` 决定提示词用哪种语言写：
    /// - `HintLang::English`：通用 chat 模型（Qwen、Llama 等）走这条
    /// - `HintLang::Chinese`：Hy-MT2 这类**按中文模板训练**的模型
    ///
    /// 为什么必须区分：实测给 Hy-MT2-1.8B 追加英文纠正提示，它会把提示词
    /// **当成待翻译的内容翻成中文**吐出来——`API` 那次就返回了整段中文说明。
    /// 中文提示它才会当作指令执行。
    pub fn retry_hint(&self, hint_lang: HintLang) -> Option<String> {
        if self.issues.is_empty() {
            return None;
        }

        let zh = hint_lang == HintLang::Chinese;
        let mut lines: Vec<String> = Vec::new();

        for issue in &self.issues {
            match issue {
                Issue::Empty => lines.push(if zh {
                    "上一次回答是空的。你必须输出翻译结果，不能什么都不给。".into()
                } else {
                    "Your previous answer was empty. You must output the translated text."
                        .into()
                }),
                Issue::TooShort { .. } => lines.push(if zh {
                    "上一次回答比原文短太多，看起来被截断了。请把每一句都完整翻译出来。".into()
                } else {
                    "Your previous answer was far shorter than the source and looks truncated. \
                     Translate every sentence completely."
                        .into()
                }),
                Issue::TooLong { .. } => lines.push(if zh {
                    "上一次回答比原文长太多。只输出译文，不要复读、不要加说明。".into()
                } else {
                    "Your previous answer was far longer than the source. Output only the \
                     translation, with no repetition, notes or explanations."
                        .into()
                }),
                Issue::Structure(kinds) => {
                    let mut kept: Vec<&str> = Vec::new();
                    for kind in kinds {
                        match *kind {
                            "列表" => kept.push(if zh {
                                "保留每个行首的列表符号（`-`、`*`、`1.`）"
                            } else {
                                "keep every list marker (`-`, `*`, `1.`) at the line start"
                            }),
                            "代码块" => kept.push(if zh {
                                "保留 ``` 代码围栏"
                            } else {
                                "keep the ``` code fences"
                            }),
                            "时间轴" => kept.push(if zh {
                                "保留字幕时间轴，例如 `00:00:01,000 --> 00:00:03,500`"
                            } else {
                                "keep the subtitle timestamps such as `00:00:01,000 --> 00:00:03,500`"
                            }),
                            _ => {}
                        }
                    }
                    lines.push(if zh {
                        format!("原文里有这些结构，你的回答弄丢了：{}。", kept.join("；"))
                    } else {
                        format!(
                            "The source has extra structure that your answer lost: {}.",
                            kept.join("; ")
                        )
                    });
                }
                Issue::Glossary(terms) => {
                    let list = terms
                        .iter()
                        .map(|t| format!("`{t}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    lines.push(if zh {
                        format!("这些术语对照被忽略了，必须严格按它翻译：{list}。")
                    } else {
                        format!(
                            "These term mappings were ignored, use them exactly: {list}."
                        )
                    });
                }
                Issue::Untranslated => lines.push(if zh {
                    "上一次回答和原文一模一样，等于没有翻译。必须用目标语言给出译文，\
                     即使输入只有一个词也要翻。只有专有名词、代码，以及目标语言中习惯上\
                     直接沿用原文的说法，才可以保持原样。"
                        .into()
                } else {
                    "Your previous answer was identical to the source text — nothing was \
                     translated. You must output the translation in the target language, \
                     even for very short or single-word input. Only leave text unchanged \
                     when it is a proper noun, code, or a term that is conventionally \
                     written the same way in the target language."
                        .into()
                }),
            }
        }

        lines.push(if zh {
            "只输出修正后的译文，不要任何解释。".into()
        } else {
            "Output the corrected translation only.".into()
        });
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

// ============================================================================
// "原文没被翻译" 的判定
// ============================================================================
//
// 思路移植自 Weblate 的 `Unchanged translation` 检查（GPL-3.0，只借鉴算法）：
// https://github.com/WeblateOrg/weblate/blob/weblate-4.10.1/weblate/checks/same.py
//
// 核心是**逐词判断**而不是整句白名单：
//   - 只要句子里有一个"真正需要翻译"的词，就不允许原样返回
//   - `API` → 忽略（缩写）
//   - `Hello world` → 必须翻（hello 不在白名单）
//   - `PDF export` → 必须翻（export 不在白名单）
//
// 这比维护一张"哪些句子可以原样"的清单可靠得多，而且天然跨语言通用。

/// 常出现在译文里但**本来就该保持原样**的词（小写）。
///
/// 取自 Weblate 的词表并精简：只保留真正高频、
/// 在多数语言里都直接沿用原文的技术/通用词。
/// 长度 ≤ 2 的词不用进表（见 [`is_ignorable_word`]）。
const IGNORE_WORDS: &[&str] = &[
    // 协议 / 格式 / 标准
    "api", "url", "uri", "http", "https", "ftp", "ssh", "smtp", "dns", "tcp", "udp", "ip",
    "html", "css", "xml", "json", "yaml", "toml", "sql", "csv", "pdf", "png", "jpg", "jpeg",
    "gif", "svg", "zip", "tar", "exe", "dll", "iso", "mp3", "mp4", "wav", "mkv", "avi",
    // 硬件 / 系统
    "cpu", "gpu", "ram", "rom", "usb", "ssd", "hdd", "bios", "os", "pc", "mac", "ios",
    "android", "linux", "windows", "ubuntu", "docker", "kubernetes", "git", "github",
    // 软件 / 服务（只收"几乎没有普通含义"的品牌名；apple、windows 这类
    // 本身也是日常词汇，放进来会让 "Apple" 这种水果不翻译，所以不收）
    "app", "apps", "sdk", "ide", "cli", "gui", "ui", "ux", "ai", "ml", "llm", "gpt",
    "chatgpt", "openai", "github", "aws", "azure",
    "facebook", "twitter", "instagram", "youtube", "tiktok", "wechat", "alipay",
    // 常见界面词（很多语言直接沿用英文）
    "email", "e-mail", "sms", "ok", "id", "uid", "uuid", "gps", "wifi", "bluetooth",
    "nfc", "vpn", "cdn", "dns", "ssl", "tls", "oauth", "jwt", "sso", "2fa", "otp",
    // 单位 / 量词
    "mb", "gb", "tb", "kb", "hz", "khz", "mhz", "ghz", "px", "dpi", "fps", "ms", "ns",
];

/// 这个词是不是"不需要翻译"。
///
/// 四条规则：
/// 1. 长度 ≤ 2：`OK`、`ID`、`AI` 这类多半是缩写
/// 2. 纯数字：`100`、`35`、`2026` 数字跨语言通常不变
/// 3. 全大写：`README`、`TODO` 这类通常是代码标识符
/// 4. 在白名单里
fn is_ignorable_word(word: &str) -> bool {
    if word.chars().count() <= 2 {
        return true;
    }
    // 纯数字（允许小数点、逗号、百分号混在词里也不该算"要翻"）
    if word.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    let all_upper =
        word.chars().all(|c| !c.is_lowercase()) && word.chars().any(|c| c.is_uppercase());
    if all_upper {
        return true;
    }
    IGNORE_WORDS.contains(&word.to_lowercase().as_str())
}

/// 去掉那些"不该参与比对"的成分，再判断是否还需要翻译。
///
/// 剥离顺序照着 Weblate 来：HTML 标签 → 占位符 → emoji → 邮箱 → URL →
/// 主机名 → 路径 → 模板标记。最后只剩"真正要翻的内容"。
fn strip_for_compare(text: &str) -> String {
    let mut s = text.to_string();

    // HTML / XML 标签
    s = strip_tags(&s);
    // 占位符：{name} {{name}} %s %d %1$s ${name} {0}
    s = strip_placeholders(&s);
    // URL / 邮箱 / 域名
    s = strip_links(&s);
    // 文件路径（/usr/local/bin、C:\Users\...）
    s = strip_paths(&s);

    s
}

/// 去掉 `<tag ...>` 与 `</tag>`。
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// 去掉常见占位符形态。用一遍扫描而不是正则，避免引入依赖。
fn strip_placeholders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // ${...} / {{...}} / {...}
        if c == '$' && i + 1 < chars.len() && chars[i + 1] == '{' {
            if let Some(end) = find_closing(&chars, i + 1, '{', '}') {
                i = end + 1;
                continue;
            }
        }
        if c == '{' {
            if let Some(end) = find_closing(&chars, i, '{', '}') {
                i = end + 1;
                continue;
            }
        }
        // %s %d %1$s %(name)s
        if c == '%' && i + 1 < chars.len() {
            let mut j = i + 1;
            // 位置参数 1$ 2$ 之类
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j < chars.len() && chars[j] == '$' {
                j += 1;
            }
            // 括号形式 %(name)s
            if j < chars.len() && chars[j] == '(' {
                if let Some(close) = find_closing(&chars, j, '(', ')') {
                    j = close + 1;
                    if j < chars.len() && chars[j].is_ascii_alphabetic() {
                        j += 1;
                    }
                    i = j;
                    continue;
                }
            }
            // 简单形式：%s %d %f %.2f
            if j < chars.len() && (chars[j].is_ascii_alphabetic() || chars[j] == '.') {
                let mut k = j;
                while k < chars.len() && (chars[k] == '.' || chars[k].is_ascii_digit()) {
                    k += 1;
                }
                if k < chars.len() && chars[k].is_ascii_alphabetic() {
                    i = k + 1;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// 从 `open` 位置开始找配对的闭合符号，返回闭合位置。
fn find_closing(chars: &[char], open: usize, opener: char, closer: char) -> Option<usize> {
    if chars.get(open) != Some(&opener) {
        return None;
    }
    let mut depth = 0usize;
    for (idx, ch) in chars.iter().enumerate().skip(open) {
        if *ch == opener {
            depth += 1;
        } else if *ch == closer {
            depth -= 1;
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

/// 去掉 URL、邮箱、裸域名。
fn strip_links(text: &str) -> String {
    text.split_whitespace()
        .filter(|w| {
            let lower = w.to_lowercase();
            let is_url = lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("ftp://")
                || lower.starts_with("www.");
            let is_email = w.contains('@') && w.contains('.');
            !(is_url || is_email)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 去掉看起来像路径的词。
fn strip_paths(text: &str) -> String {
    text.split_whitespace()
        .filter(|w| {
            let slashes = w.matches('/').count() + w.matches('\\').count();
            // 含多个斜杠的基本是路径；单个斜杠可能是 "and/or"，保留
            slashes < 2
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// 判断这次"译文=原文"该不该放过。
///
/// 返回 `true` 表示**应当忽略**（原样返回是合理的），
/// 返回 `false` 表示这是一次真正的"没翻译"，要报 [`Issue::Untranslated`]。
fn should_ignore_untranslated(source: &str) -> bool {
    // 纯数字、标点、符号：没什么可翻的
    let stripped = strip_for_compare(source);
    let meaningful: String = stripped
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    if meaningful.chars().count() <= 1 {
        return true;
    }

    // 逐词判断：只要有一个词需要翻译，就不能原样返回
    let words: Vec<&str> = stripped
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();

    if words.is_empty() {
        return true;
    }

    // 全部词都可忽略 → 放过
    words.iter().all(|w| is_ignorable_word(w))
}

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

    // 原样返回：放在长度判断之前，因为它是最明确的一种失败。
    // 长度比例是 1.0、无结构问题、无术语要求，后面所有规则都会放行，
    // 所以必须单独抓。是否该放过（`API` 这类）由 should_ignore 决定。
    if trimmed == source.trim() && !should_ignore_untranslated(source) {
        issues.push(Issue::Untranslated);
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
        let hint = report.retry_hint(HintLang::English).expect("有问题就该给出提示");
        assert!(hint.contains("list marker"), "提示应点名列表：{hint}");
        assert!(hint.contains("apple => 苹果"), "提示应带上具体术语：{hint}");

        assert!(check("hello", "你好", &[]).retry_hint(HintLang::English).is_none());
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

    // ---- 原样返回（Untranslated）----
    //
    // 这组测试的意义：既要把"模型偷懒"抓住，也不能冤枉本来就不该翻的词。
    // 每条都对应一个真实场景。

    /// 短词被原样返回，必须抓住——这是用户实测报告的原始 bug。
    #[test]
    fn unchanged_short_words_are_flagged() {
        for text in ["hello", "Apple", "goodbye", "Hello world", "thank you"] {
            let report = check(text, text, &[]);
            assert!(
                report.issues.contains(&Issue::Untranslated),
                "「{text}」原样返回应当被检出，实际: {:?}",
                report.issues
            );
        }
    }

    /// 缩写与代码标识符原样保留是**对的**，不能报错。
    #[test]
    fn abbreviations_and_codes_are_not_flagged() {
        for text in [
            "API",
            "OK",
            "PDF",
            "CPU",
            "URL",
            "ID",
            "README",
            "TODO",
            "https://example.com",
            "user@example.com",
            "{{name}}",
            "{count}",
            "%s",
            "C:\\Users\\file.txt",
        ] {
            let report = check(text, text, &[]);
            assert!(
                !report.issues.contains(&Issue::Untranslated),
                "「{text}」不该被当成未翻译，实际: {:?}",
                report.issues
            );
        }
    }

    /// 逐词判断：句子里只要有一个"该翻的词"，就必须翻。
    ///
    /// 这是与"整句白名单"最关键的区别——`PDF export` 里 `pdf` 虽在白名单，
    /// 但 `export` 需要翻译，所以不能放过。
    #[test]
    fn one_translatable_word_is_enough_to_flag() {
        for text in ["PDF export", "API key settings", "OK to proceed"] {
            let report = check(text, text, &[]);
            assert!(
                report.issues.contains(&Issue::Untranslated),
                "「{text}」含需要翻译的词，应当被检出，实际: {:?}",
                report.issues
            );
        }
    }

    /// 纯数字/标点/符号没什么可翻的，原样返回合理。
    #[test]
    fn punctuation_and_numbers_are_not_flagged() {
        for text in ["123", "1.5", "---", "=>", "。", "...", "100%"] {
            let report = check(text, text, &[]);
            assert!(
                !report.issues.contains(&Issue::Untranslated),
                "「{text}」没有可翻内容，不该报未翻译"
            );
        }
    }

    /// 真的翻了就不该报（译文与原文不同）。
    #[test]
    fn actual_translation_is_not_flagged() {
        let report = check("hello", "你好", &[]);
        assert!(!report.issues.contains(&Issue::Untranslated));
        let report = check("Hello world", "你好，世界", &[]);
        assert!(!report.issues.contains(&Issue::Untranslated));
    }

    /// 剥离占位符后再比对：带占位符的句子如果只剩占位符，放过。
    ///
    /// 反过来，`Hello {name}` 原样返回时，剥离后还剩 `Hello`，仍要报。
    #[test]
    fn placeholders_are_stripped_before_comparing() {
        // 只剩占位符 → 放过
        assert!(!check("{{count}}", "{{count}}", &[])
            .issues
            .contains(&Issue::Untranslated));
        // 占位符之外还有实词 → 报
        assert!(check("Hello {{name}}", "Hello {{name}}", &[])
            .issues
            .contains(&Issue::Untranslated));
    }

    /// 重试提示必须明确说清"你在偷懒"，这是让它改对的关键。
    #[test]
    fn untranslated_retry_hint_says_what_went_wrong() {
        let report = check("hello", "hello", &[]);
        let hint = report.retry_hint(HintLang::English).expect("应当有重试提示");
        assert!(
            hint.to_lowercase().contains("identical to the source"),
            "提示要指出原样返回：{hint}"
        );
    }

    /// 中文模型必须拿到中文纠正提示。
    ///
    /// 实测给 Hy-MT2-1.8B 追加英文提示，它会把提示词本身翻成中文当输出；
    /// 中文提示才会被当成指令执行。
    #[test]
    fn chinese_hint_stays_chinese() {
        let report = check("hello", "hello", &[]);
        let zh = report
            .retry_hint(HintLang::Chinese)
            .expect("应当有重试提示");

        assert!(
            zh.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "中文提示应当含中文：{zh}"
        );
        assert!(zh.contains("没有翻译"), "应当指出没翻译：{zh}");
        assert!(
            !zh.contains("identical to the source"),
            "中文提示不该混入英文原句：{zh}"
        );
        assert!(zh.contains("只输出修正后的译文"), "{zh}");
    }

    /// 两种语言必须真的不同，否则等于没做区分。
    #[test]
    fn hint_language_actually_changes_output() {
        let report = check("hello", "hello", &[]);
        let en = report.retry_hint(HintLang::English).unwrap();
        let zh = report.retry_hint(HintLang::Chinese).unwrap();
        assert_ne!(en, zh);
    }
}
