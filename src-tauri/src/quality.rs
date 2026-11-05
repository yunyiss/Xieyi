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
    /// 译文的文字系统跟目标语言对不上（要中文却给英文、要俄语却给拉丁字母…）。
    ///
    /// 与 [`Issue::Untranslated`] 的区别：那条是"一个字都没改"，
    /// 这条是"确实换成了别的东西，但换错了语言"。
    /// 典型场景：模型不支持目标语言，退回到它最熟的英语。
    WrongScript {
        /// 目标语言要求的文字系统名，用于提示
        expected: &'static str,
    },
    /// 目标语言是简体/繁体之一，但译文用的是另一种字形。
    ///
    /// 只有目标为 `zh`（简体）或 `zh-tw`（繁体）时才检查，
    /// 其他语言没有这种"同语言两种字形"的问题。
    WrongVariant {
        /// 目标要求的是简体还是繁体
        expected: &'static str,
        /// 实际检出另一种字形的字数
        found: usize,
    },
    /// 译文里混进了我们发出去的**指令**。
    ///
    /// 纯翻译模型（translategemma 这类）的模板就是"把收到的全部内容翻成目标语言"，
    /// 没有给"额外要求"留位置。我们把风格要求以 `[Note: ...]` 附在原文后面，
    /// 它会把这段附注当成待翻译内容，翻出来拼在译文里：
    ///
    /// ```text
    /// 输入：The server is down.\n\n[Note: This is technical content...]
    /// 输出：服务器已停止运行。\n\n[注意：这属于技术内容：...]
    /// ```
    ///
    /// 实测 4b 模型下，`formal`/`literary` 侥幸没触发，但 `technical`/`subtitle`
    /// 三次全中，`Keep code unchanged.` 甚至两个词的 `Technical.` 也会被翻。
    /// 所以这不是"提示太长"，是"任何附加内容"。
    ///
    /// 检测这条不需要知道模型支不支持风格——这正是它的价值：
    /// 未适配过的新模型犯同样的错，一样能被抓住。
    InstructionLeak,
}

impl Issue {
    /// 稳定的原因码，供前端按类型区分展示。
    ///
    /// 为什么不让前端解析 `describe()` 的中文：文案会改，解析会脆。
    /// 这里给机器读的固定标识，`describe()` 给人和界面读的说明。
    pub fn code(&self) -> &'static str {
        match self {
            Issue::Empty => "empty",
            Issue::TooShort { .. } => "too-short",
            Issue::TooLong { .. } => "too-long",
            Issue::Structure(_) => "structure",
            Issue::Glossary(_) => "glossary",
            Issue::Untranslated => "untranslated",
            Issue::WrongScript { .. } => "wrong-script",
            Issue::WrongVariant { .. } => "wrong-variant",
            Issue::InstructionLeak => "instruction-leak",
        }
    }

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
            Issue::WrongScript { expected } => {
                format!("译文不是{expected}，可能翻成了其他语言")
            }
            Issue::WrongVariant { expected, found } => {
                format!("目标为{expected}，但译文检出 {found} 处另一种字形")
            }
            Issue::InstructionLeak => "译文里混入了指令内容，模型把附注也翻了出来".into(),
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

    /// 本次检出的问题码（去重）。前端据此决定展示哪条提示。
    pub fn codes(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for issue in &self.issues {
            let c = issue.code();
            if !out.contains(&c) {
                out.push(c);
            }
        }
        out
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
                Issue::WrongScript { expected } => lines.push(if zh {
                    format!(
                        "上一次的译文不是{expected}，可能翻成了其他语言。请用{expected}重新翻译。"
                    )
                } else {
                    "Your previous answer was not written in the expected target language. Re-translate it properly.".to_string()
                }),
                Issue::WrongVariant { expected, .. } => lines.push(if zh {
                    format!(
                        "上一次的译文用错了字形，目标是{expected}。请用{expected}的标准字形重新输出。"
                    )
                } else {
                    "Your previous answer used the wrong character variants for the target language. Use the standard forms."
                        .into()
                }),
                Issue::InstructionLeak => lines.push(if zh {
                    "上一次的输出把说明文字也翻译出来了。只翻译正文，不要包含任何方括号里的说明。"
                        .into()
                } else {
                    "Your previous answer included instruction text that was translated along \
                     with the content. Output only the translation of the source text — never \
                     include bracketed notes, labels or instructions."
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
    "api",
    "url",
    "uri",
    "http",
    "https",
    "ftp",
    "ssh",
    "smtp",
    "dns",
    "tcp",
    "udp",
    "ip",
    "html",
    "css",
    "xml",
    "json",
    "yaml",
    "toml",
    "sql",
    "csv",
    "pdf",
    "png",
    "jpg",
    "jpeg",
    "gif",
    "svg",
    "zip",
    "tar",
    "exe",
    "dll",
    "iso",
    "mp3",
    "mp4",
    "wav",
    "mkv",
    "avi",
    // 硬件 / 系统
    "cpu",
    "gpu",
    "ram",
    "rom",
    "usb",
    "ssd",
    "hdd",
    "bios",
    "os",
    "pc",
    "mac",
    "ios",
    "android",
    "linux",
    "windows",
    "ubuntu",
    "docker",
    "kubernetes",
    "git",
    "github",
    // 软件 / 服务（只收"几乎没有普通含义"的品牌名；apple、windows 这类
    // 本身也是日常词汇，放进来会让 "Apple" 这种水果不翻译，所以不收）
    "app",
    "apps",
    "sdk",
    "ide",
    "cli",
    "gui",
    "ui",
    "ux",
    "ai",
    "ml",
    "llm",
    "gpt",
    "chatgpt",
    "openai",
    "github",
    "aws",
    "azure",
    "facebook",
    "twitter",
    "instagram",
    "youtube",
    "tiktok",
    "wechat",
    "alipay",
    // 常见界面词（很多语言直接沿用英文）
    "email",
    "e-mail",
    "sms",
    "ok",
    "id",
    "uid",
    "uuid",
    "gps",
    "wifi",
    "bluetooth",
    "nfc",
    "vpn",
    "cdn",
    "dns",
    "ssl",
    "tls",
    "oauth",
    "jwt",
    "sso",
    "2fa",
    "otp",
    // 单位 / 量词
    "mb",
    "gb",
    "tb",
    "kb",
    "hz",
    "khz",
    "mhz",
    "ghz",
    "px",
    "dpi",
    "fps",
    "ms",
    "ns",
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
        if c == '$'
            && i + 1 < chars.len()
            && chars[i + 1] == '{'
            && let Some(end) = find_closing(&chars, i + 1, '{', '}')
        {
            i = end + 1;
            continue;
        }
        if c == '{'
            && let Some(end) = find_closing(&chars, i, '{', '}')
        {
            i = end + 1;
            continue;
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
            if j < chars.len()
                && chars[j] == '('
                && let Some(close) = find_closing(&chars, j, '(', ')')
            {
                j = close + 1;
                if j < chars.len() && chars[j].is_ascii_alphabetic() {
                    j += 1;
                }
                i = j;
                continue;
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
    let meaningful: String = stripped.chars().filter(|c| c.is_alphanumeric()).collect();
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
                !digits.is_empty() && digits.len() <= 3 && t[digits.len()..].starts_with(". ")
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

// ============================================================================
// 语言一致性：译文到底是不是目标语言
// ============================================================================
//
// 解决的问题：模型不支持目标语言时，会**静默退回到它最熟的语言**。
// 典型例子是中文模型把日语翻成中文、或小模型把繁体翻成简体——
// 长度、结构、术语全都正常，只有"语言不对"这一项过不去。
//
// 做法是**按文字系统**（script）判断，而不是做语言识别：
//   - 要中文却一个汉字都没有 → 明显错了
//   - 要日语却只有汉字没有假名 → 大概率翻成了中文
//   - 要简体却满是繁体字 → 字形不对
//
// 只做"文字系统"这一层，不引入语言识别模型：零依赖、微秒级，
// 能抓住绝大多数"退回到别的语言"，而不会误伤"译文里夹了英文术语"。

/// 一个语言要求什么文字系统。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    /// 汉字（简体、繁体都算）
    Han,
    /// 日语：汉字 + 假名，**必须有假名**
    Japanese,
    /// 韩语：谚文
    Hangul,
    /// 拉丁字母
    Latin,
    /// 西里尔字母
    Cyrillic,
    /// 阿拉伯字母
    Arabic,
    /// 泰文
    Thai,
    /// 不认识的语言，不做检查
    Unknown,
}

/// 目标语言 → 应该出现什么文字。
fn script_of(lang: &str) -> Script {
    match lang {
        "zh" | "zh-tw" | "zh-hant" => Script::Han,
        "ja" => Script::Japanese,
        "ko" => Script::Hangul,
        "en" | "fr" | "de" | "es" | "pt" | "it" | "vi" => Script::Latin,
        "ru" => Script::Cyrillic,
        "ar" => Script::Arabic,
        "th" => Script::Thai,
        _ => Script::Unknown,
    }
}

/// 文字系统在提示语里的中文名。
fn script_label(script: Script) -> &'static str {
    match script {
        Script::Han => "中文",
        Script::Japanese => "日文",
        Script::Hangul => "韩文",
        Script::Latin => "拉丁字母文字",
        Script::Cyrillic => "西里尔字母文字",
        Script::Arabic => "阿拉伯文",
        Script::Thai => "泰文",
        Script::Unknown => "目标语言",
    }
}

fn is_han(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)      // CJK 统一表意文字
        || ('\u{3400}'..='\u{4dbf}').contains(&c) // 扩展 A
        || ('\u{f900}'..='\u{faff}').contains(&c) // 兼容表意文字
}

fn is_kana(c: char) -> bool {
    ('\u{3040}'..='\u{309f}').contains(&c) || ('\u{30a0}'..='\u{30ff}').contains(&c)
}

fn is_hangul(c: char) -> bool {
    ('\u{ac00}'..='\u{d7af}').contains(&c) || ('\u{1100}'..='\u{11ff}').contains(&c)
}

fn is_latin(c: char) -> bool {
    c.is_ascii_alphabetic() || ('\u{00c0}'..='\u{024f}').contains(&c)
}

fn is_cyrillic(c: char) -> bool {
    ('\u{0400}'..='\u{04ff}').contains(&c)
}

fn is_arabic(c: char) -> bool {
    ('\u{0600}'..='\u{06ff}').contains(&c)
}

fn is_thai(c: char) -> bool {
    ('\u{0e00}'..='\u{0e7f}').contains(&c)
}

/// 经常用的繁简差异字（简体形 → 繁体形）。
///
/// 不追求完整（完整有两千多对），只收**高频**的：
/// 只要一段译文里出现其中几个繁体形，就足以判断字形不对。
/// 反过来，简体形出现在繁体译文里同理。
const HAN_SIMPLIFIED: &[char] = &[
    '设', '历', '网', '络', '简', '体', '关', '闭', '开', '启', '档', '案', '数', '据', '库', '这',
    '个', '为', '与', '说', '时', '会', '经', '认', '证', '错', '误', '验', '测', '试', '现', '点',
    '击', '选', '择', '项', '图', '标', '页', '面', '显', '示', '隐', '藏', '复', '制', '粘', '贴',
    '删', '除', '修', '改', '查', '询', '过', '滤', '导', '出', '总', '结', '报', '表', '统', '计',
    '计', '算', '应', '用', '软', '件', '系', '统', '机', '器', '学',
];

const HAN_TRADITIONAL: &[char] = &[
    '設', '歷', '網', '絡', '簡', '體', '關', '閉', '開', '啟', '檔', '案', '數', '據', '庫', '這',
    '個', '為', '與', '說', '時', '會', '經', '認', '證', '錯', '誤', '驗', '測', '試', '現', '點',
    '擊', '選', '擇', '項', '圖', '標', '頁', '面', '顯', '示', '隱', '藏', '複', '製', '粘', '貼',
    '刪', '除', '修', '改', '查', '詢', '過', '濾', '導', '出', '總', '結', '報', '表', '統', '計',
    '計', '算', '應', '用', '軟', '件', '系', '統', '機', '器', '學',
];

/// 数一数文本里出现了几个指定集合的字。
fn count_chars_in(text: &str, set: &[char]) -> usize {
    text.chars().filter(|c| set.contains(c)).count()
}

/// 检查译文用的文字系统对不对。
///
/// 只报"明显不对"的情况：
/// - 目标文字系统一个字都没有（要中文却全英文）
/// - 日语没有假名（翻成了中文或英文）
///
/// **故意不报"混入了少量其他文字"**：技术文档里夹英文太常见了，
/// 那是正常的，不是翻译失败。
fn check_script(target_lang: &str, translated: &str) -> Option<Issue> {
    let script = script_of(target_lang);
    if script == Script::Unknown {
        return None;
    }

    let chars: Vec<char> = translated.chars().filter(|c| c.is_alphanumeric()).collect();
    if chars.is_empty() {
        return None; // 空输出由 Empty 管
    }

    match script {
        Script::Han => {
            if !chars.iter().any(|c| is_han(*c)) {
                return Some(Issue::WrongScript {
                    expected: script_label(script),
                });
            }
        }
        Script::Japanese => {
            // 日语必须出现假名。只有汉字说明多半翻成了中文。
            let has_kana = chars.iter().any(|c| is_kana(*c));
            let has_han = chars.iter().any(|c| is_han(*c));
            if !has_kana && has_han {
                return Some(Issue::WrongScript {
                    expected: script_label(script),
                });
            }
        }
        Script::Hangul => {
            if !chars.iter().any(|c| is_hangul(*c)) {
                return Some(Issue::WrongScript {
                    expected: script_label(script),
                });
            }
        }
        Script::Latin => {
            if !chars.iter().any(|c| is_latin(*c)) {
                return Some(Issue::WrongScript {
                    expected: script_label(script),
                });
            }
        }
        Script::Cyrillic => {
            if !chars.iter().any(|c| is_cyrillic(*c)) {
                return Some(Issue::WrongScript {
                    expected: script_label(script),
                });
            }
        }
        Script::Arabic => {
            if !chars.iter().any(|c| is_arabic(*c)) {
                return Some(Issue::WrongScript {
                    expected: script_label(script),
                });
            }
        }
        Script::Thai => {
            if !chars.iter().any(|c| is_thai(*c)) {
                return Some(Issue::WrongScript {
                    expected: script_label(script),
                });
            }
        }
        Script::Unknown => {}
    }
    None
}

/// 检查译文里有没有混进我们发出去的指令。
///
/// 我们给风格/术语/纠正提示加的是 `[Note: ...]` `[Glossary: ...]`
/// `[Correction: ...]` 这类方括号标记。正常译文里几乎不会出现
/// 结构化方括号（[1] 这种引用除外），所以"输出里出现方括号标记"
/// 是相当强的信号。
///
/// 为什么不逐字比对指令原文：模型会把它**翻译**成目标语言
/// （`[Note: keep code unchanged]` → `[注意：请保持代码不变]`），
/// 原文根本对不上。只能看标记形态。
fn check_instruction_leak(translated: &str) -> Option<Issue> {
    // 我们自己发出的标记（英文原形，及各语言翻译后的常见形态）
    const MARKERS: &[&str] = &[
        "[note:",
        "[注意：",
        "[注意:",
        "[glossary:",
        "[术语",
        "[correction:",
        "[纠正",
    ];

    let lower = translated.to_lowercase();
    // 统一转小写后做子串比对：英文标记因此大小写不敏感；
    // 中文标记没有大小写概念，`to_lowercase` 对它们是恒等变换，
    // 所以一条规则就能覆盖两种情况。
    //
    // 早先这里写成 `if marker.is_ascii() { marker.to_string() } else { marker.to_string() }`
    // ——两个分支完全相同，判断毫无作用（clippy `if_same_then_else` 抓出来的）。
    // 行为上没差别，但会让读代码的人以为中英文走了不同路径，白白多花时间确认。
    for marker in MARKERS {
        if lower.contains(&marker.to_lowercase()) {
            return Some(Issue::InstructionLeak);
        }
    }

    // 兜底：中文方括号里出现"注意/术语/纠正"这类词。
    // 译成日文/韩文时形态不同，这条只管中文。
    for segment in translated.split('[').skip(1) {
        if let Some(end) = segment.find([']', '】']) {
            let inner = &segment[..end];
            if inner.contains("注意")
                || inner.contains("術語")
                || inner.contains("术语")
                || inner.contains("糾正")
                || inner.contains("纠正")
            {
                return Some(Issue::InstructionLeak);
            }
        }
    }

    None
}

/// 检查简繁字形对不对。
///
/// 只在目标是简体（`zh`）或繁体（`zh-tw`）时检查。
/// 判定用**数量对比**而不是"出现即报"：某些字在两种字形里都合法，
/// 要看整体偏向哪一边。
fn check_variant(target_lang: &str, translated: &str) -> Option<Issue> {
    match target_lang {
        "zh" => {
            // 要简体：繁体字明显多于简体字才算错
            let simp = count_chars_in(translated, HAN_SIMPLIFIED);
            let trad = count_chars_in(translated, HAN_TRADITIONAL);
            if trad > simp && trad >= 2 {
                return Some(Issue::WrongVariant {
                    expected: "简体中文",
                    found: trad,
                });
            }
        }
        "zh-tw" | "zh-hant" => {
            // 要繁体：简体字明显多于繁体字才算错。
            // 注意繁体译文里也可能出现简体形（有些字本来就没差异），
            // 所以要求"简体确实占多数且有 2 个以上"。
            let simp = count_chars_in(translated, HAN_SIMPLIFIED);
            let trad = count_chars_in(translated, HAN_TRADITIONAL);
            if simp > trad && simp >= 2 {
                return Some(Issue::WrongVariant {
                    expected: "繁体中文",
                    found: simp,
                });
            }
        }
        _ => {}
    }
    None
}

/// 对一个片段的译文做全套检查。
///
/// `target_lang` 是目标语言代码（`zh` / `zh-tw` / `ja`…），用于判断
/// 译文用的文字系统对不对。传空串或未知语言时跳过这项检查。
pub fn check(source: &str, target: &str, terms: &[Term], target_lang: &str) -> Report {
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

    // 语言一致性：模型不支持目标语言时会静默退回它最熟的语言，
    // 长度和结构都正常，只有"语言不对"这一项能发现。
    if let Some(issue) = check_script(target_lang, trimmed) {
        issues.push(issue);
    }
    if let Some(issue) = check_variant(target_lang, trimmed) {
        issues.push(issue);
    }

    // 指令泄漏：纯翻译模型会把我们加的附注当正文一起翻出来。
    // 与语言无关，任何模型任何语言都可能中招，所以放在通用检查里。
    if let Some(issue) = check_instruction_leak(trimmed) {
        issues.push(issue);
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
pub fn better_than(
    candidate: &str,
    candidate_report: &Report,
    first: &str,
    first_report: &Report,
) -> bool {
    match candidate_report
        .issues
        .len()
        .cmp(&first_report.issues.len())
    {
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
        pairs.iter().map(|(from, to)| Term { from, to }).collect()
    }

    #[test]
    fn empty_or_punctuation_only_is_flagged() {
        assert!(!check("hello", "", &[], "").is_ok());
        assert!(!check("hello", "   ", &[], "").is_ok());
        assert!(!check("hello", "...", &[], "").is_ok());
        // 空输出只报一条，不叠"过短"
        assert_eq!(check("hello", "", &[], "").issues.len(), 1);
    }

    #[test]
    fn normal_matches_are_clean() {
        // 英文→中文，字数约一半
        assert!(
            check(
                "Hello world, this is a test sentence.",
                "你好世界，这是一句测试文字。",
                &[],
                ""
            )
            .is_ok()
        );

        // 中文→英文，字数翻倍也是正常的
        assert!(
            check(
                "这是一句用来测试的中文句子，长度适中。",
                "This is a Chinese sentence used for testing, and its length is moderate.",
                &[],
                ""
            )
            .is_ok()
        );
    }

    #[test]
    fn truncated_output_is_flagged() {
        let source = "这是一段比较长的说明文字，用来模拟被截断的情况。它包含多个句子，模型只翻了开头就停了。";
        let target = "这";
        let report = check(source, target, &[], "");
        assert!(matches!(report.issues[0], Issue::TooShort { .. }));
    }

    #[test]
    fn verbose_output_is_flagged() {
        let source = "Hello.";
        let target = "你好。".repeat(40);
        let report = check(source, &target, &[], "");
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, Issue::TooLong { .. }))
        );
    }

    #[test]
    fn lost_list_structure_is_flagged() {
        let source = "- apple\n- banana\n- cherry";
        let target = "苹果香蕉樱桃";
        let report = check(source, target, &[], "");
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, Issue::Structure(_)))
        );

        // 保留列表标记就不该报
        assert!(check(source, "- 苹果\n- 香蕉\n- 樱桃", &[], "").is_ok());
    }

    #[test]
    fn lost_code_fence_is_flagged() {
        let source = "用法：\n```\nrun --now\n```";
        let target = "用法：运行 --now";
        assert!(
            check(source, target, &[], "")
                .issues
                .iter()
                .any(|i| matches!(i, Issue::Structure(_)))
        );
    }

    #[test]
    fn lost_subtitle_timestamp_is_flagged() {
        let source = "1\n00:00:01,000 --> 00:00:03,500\nHello";
        let target = "1\n你好";
        let report = check(source, target, &[], "");
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, Issue::Structure(_)))
        );
    }

    #[test]
    fn arrow_without_colon_is_not_a_timestamp() {
        // 普通文本里的箭头不该被当成 srt
        assert!(check("a --> b", "甲 --> 乙", &[], "").is_ok());
    }

    #[test]
    fn missed_glossary_term_is_flagged() {
        let report = check(
            "The server is down.",
            "服务宕机了。",
            &terms(&[("server", "服务器")]),
            "",
        );
        assert!(matches!(report.issues.last(), Some(Issue::Glossary(_))));

        // 用上了指定译法就通过
        assert!(
            check(
                "The server is down.",
                "服务器宕机了。",
                &terms(&[("server", "服务器")]),
                ""
            )
            .is_ok()
        );
        // 原词没出现在原文里，不检查
        assert!(check("Hello.", "你好。", &terms(&[("server", "服务器")]), "").is_ok());
    }

    #[test]
    fn hint_mentions_the_actual_problem() {
        // 译文既丢了列表，也没用上术语表指定的"苹果"
        let report = check(
            "- apple\n- banana",
            "红果和香蕉",
            &terms(&[("apple", "苹果")]),
            "",
        );
        let hint = report
            .retry_hint(HintLang::English)
            .expect("有问题就该给出提示");
        assert!(hint.contains("list marker"), "提示应点名列表：{hint}");
        assert!(hint.contains("apple => 苹果"), "提示应带上具体术语：{hint}");

        assert!(
            check("hello", "你好", &[], "")
                .retry_hint(HintLang::English)
                .is_none()
        );
    }

    #[test]
    fn descriptions_are_specific_enough_for_the_ui() {
        let report = check("hello world", "h", &[], "");
        let text = report.describe();
        assert!(text.contains("截断"), "应说明是截断：{text}");
    }

    #[test]
    fn better_than_prefers_fewer_issues_then_longer_text() {
        let bad = check("hello world foo bar", "x", &[], "");
        let good = check("hello world foo bar", "你好世界，富吧。", &[], "");
        assert!(better_than("你好世界，富吧。", &good, "x", &bad));

        // 问题同样多时，长的那个更可能是完整输出
        let a = check("hello", "a", &[], "");
        let b = check("hello", "ab", &[], "");
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
            let report = check(text, text, &[], "");
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
            let report = check(text, text, &[], "");
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
            let report = check(text, text, &[], "");
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
            let report = check(text, text, &[], "");
            assert!(
                !report.issues.contains(&Issue::Untranslated),
                "「{text}」没有可翻内容，不该报未翻译"
            );
        }
    }

    /// 真的翻了就不该报（译文与原文不同）。
    #[test]
    fn actual_translation_is_not_flagged() {
        let report = check("hello", "你好", &[], "");
        assert!(!report.issues.contains(&Issue::Untranslated));
        let report = check("Hello world", "你好，世界", &[], "");
        assert!(!report.issues.contains(&Issue::Untranslated));
    }

    /// 剥离占位符后再比对：带占位符的句子如果只剩占位符，放过。
    ///
    /// 反过来，`Hello {name}` 原样返回时，剥离后还剩 `Hello`，仍要报。
    #[test]
    fn placeholders_are_stripped_before_comparing() {
        // 只剩占位符 → 放过
        assert!(
            !check("{{count}}", "{{count}}", &[], "")
                .issues
                .contains(&Issue::Untranslated)
        );
        // 占位符之外还有实词 → 报
        assert!(
            check("Hello {{name}}", "Hello {{name}}", &[], "")
                .issues
                .contains(&Issue::Untranslated)
        );
    }

    /// 重试提示必须明确说清"你在偷懒"，这是让它改对的关键。
    #[test]
    fn untranslated_retry_hint_says_what_went_wrong() {
        let report = check("hello", "hello", &[], "");
        let hint = report
            .retry_hint(HintLang::English)
            .expect("应当有重试提示");
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
        let report = check("hello", "hello", &[], "");
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
        let report = check("hello", "hello", &[], "");
        let en = report.retry_hint(HintLang::English).unwrap();
        let zh = report.retry_hint(HintLang::Chinese).unwrap();
        assert_ne!(en, zh);
    }

    // ---- 语言一致性 ----
    //
    // 模型不支持目标语言时会静默退回它最熟的语言。
    // 这组测试既要抓住"翻错语言"，也不能误伤"技术文档夹英文"。

    /// 要中文却一个汉字都没有 → 肯定错了。
    #[test]
    fn chinese_target_without_han_is_flagged() {
        let report = check("hello world", "hello world", &[], "zh");
        // 同时命中 Untranslated 与 WrongScript，只要能报出来就行
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, Issue::WrongScript { .. })),
            "应当报文字系统不对：{:?}",
            report.issues
        );
    }

    /// 中文目标、译文也是中文 → 不报。
    #[test]
    fn chinese_target_with_han_is_clean() {
        let report = check("hello", "你好", &[], "zh");
        assert!(
            !report
                .issues
                .iter()
                .any(|i| matches!(i, Issue::WrongScript { .. }))
        );
    }

    /// 日语必须有假名；只有汉字说明翻成了中文。
    #[test]
    fn japanese_target_without_kana_is_flagged() {
        // 只有汉字
        let report = check("good morning", "早上好", &[], "ja");
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, Issue::WrongScript { .. })),
            "只有汉字的日译应当报错：{:?}",
            report.issues
        );
        // 有假名就对了
        let ok = check("good morning", "おはようございます", &[], "ja");
        assert!(
            !ok.issues
                .iter()
                .any(|i| matches!(i, Issue::WrongScript { .. }))
        );
    }

    /// 技术文档里夹英文术语是正常的，不能因此报错。
    #[test]
    fn mixed_scripts_are_not_flagged() {
        let report = check(
            "The API returns a 404 error",
            "调用 API 返回 404 错误",
            &[],
            "zh",
        );
        assert!(
            !report
                .issues
                .iter()
                .any(|i| matches!(i, Issue::WrongScript { .. })),
            "夹英文术语不应报错：{:?}",
            report.issues
        );
    }

    /// 要简体却全是繁体 → 报。
    #[test]
    fn traditional_output_for_simplified_target_is_flagged() {
        let report = check(
            "Good morning, how are you?",
            "早上好，你最近怎麼樣？設定裡的網路關閉了。",
            &[],
            "zh",
        );
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, Issue::WrongVariant { .. })),
            "要简体给繁体应当报错：{:?}",
            report.issues
        );
    }

    /// 要繁体却全是简体 → 报。
    #[test]
    fn simplified_output_for_traditional_target_is_flagged() {
        let report = check(
            "Good morning, how are you?",
            "早上好，你最近怎么样？设定里的网络关闭了。",
            &[],
            "zh-tw",
        );
        assert!(
            report
                .issues
                .iter()
                .any(|i| matches!(i, Issue::WrongVariant { .. })),
            "要繁体给简体应当报错：{:?}",
            report.issues
        );
    }

    /// 字形对了就不报。
    #[test]
    fn correct_variant_is_clean() {
        let simp = check("good", "很好，设定里的网络关闭了", &[], "zh");
        assert!(
            !simp
                .issues
                .iter()
                .any(|i| matches!(i, Issue::WrongVariant { .. }))
        );

        let trad = check("good", "很好，設定裡的網路關閉了", &[], "zh-tw");
        assert!(
            !trad
                .issues
                .iter()
                .any(|i| matches!(i, Issue::WrongVariant { .. }))
        );
    }

    /// 不认识的语言不做检查，避免误伤。
    #[test]
    fn unknown_language_skips_script_check() {
        let report = check("hello", "你好", &[], "");
        assert!(
            !report
                .issues
                .iter()
                .any(|i| matches!(i, Issue::WrongScript { .. }))
        );
        let report = check("hello", "你好", &[], "xx");
        assert!(
            !report
                .issues
                .iter()
                .any(|i| matches!(i, Issue::WrongScript { .. }))
        );
    }

    /// 翻错语言时的重试提示要说清楚。
    #[test]
    fn wrong_script_hint_is_specific() {
        let report = check("hello world", "hello world", &[], "zh");
        let hint = report.retry_hint(HintLang::English).expect("应当有提示");
        assert!(
            hint.to_lowercase().contains("target language")
                || hint.to_lowercase().contains("not written in"),
            "提示要说明语言不对：{hint}"
        );
    }

    // ---- 指令泄漏（InstructionLeak）----
    //
    // 纯翻译模型会把我们加的附注当正文一起翻出来。
    // 这组测试的难点不在"抓住自己发出的标记"，而在"不误伤正常译文里的方括号"。

    /// 实测得到的真实污染输出，必须抓住。
    #[test]
    fn leaked_instruction_is_flagged() {
        let cases = [
            // 实测：translategemma-4b + technical 风格
            "服务器已停止运行。\n\n[注意：这属于技术内容：请务必保留产品名称、代码标识、数字和术语的原文；优先考虑准确性，而非流畅性。]",
            // 实测：极短提示也会被翻
            "服务器已停止工作。\n\n[注意：技术相关]",
            // 英文原形没被翻的情况
            "服务器已停止工作。\n\n[Note: keep code unchanged]",
            // 术语表泄漏
            "服务器已停止工作。[术语：server=服务器]",
        ];
        for text in cases {
            let report = check("The server is down.", text, &[], "zh");
            assert!(
                report.issues.contains(&Issue::InstructionLeak),
                "应当检出指令泄漏：{text:?}"
            );
        }
    }

    /// 正常译文里的方括号不能误伤。
    ///
    /// 这条比上一条重要：误报会让正常翻译被拒收。
    #[test]
    fn normal_brackets_are_not_flagged() {
        let cases = [
            // 引用标记
            "详见文档[1]和[2]。",
            // 代码下标
            "调用 array[0] 取第一个元素。",
            // 普通方括号内容
            "这是一段说明（如下）。",
            // 含"注意"但不在方括号里
            "请注意安全。",
        ];
        for text in cases {
            let report = check("Some source text here.", text, &[], "zh");
            assert!(
                !report.issues.contains(&Issue::InstructionLeak),
                "不应当误报：{text:?}"
            );
        }
    }

    /// 英文译文里的方括号同样不上报。
    #[test]
    fn english_brackets_are_not_flagged() {
        let cases = [
            "See the documentation [1] for details.",
            "The array[0] holds the first element.",
            "This is a normal sentence (with parentheses).",
        ];
        for text in cases {
            let report = check("一些中文原文。", text, &[], "en");
            assert!(
                !report.issues.contains(&Issue::InstructionLeak),
                "不应当误报：{text:?}"
            );
        }
    }

    /// 泄漏时的重试提示要点名“不要包含方括号说明”。
    #[test]
    fn instruction_leak_hint_is_specific() {
        let report = check(
            "The server is down.",
            "服务器已停止工作。[注意：技术内容]",
            &[],
            "zh",
        );
        let en = report.retry_hint(HintLang::English).expect("应当有提示");
        assert!(
            en.contains("bracketed") || en.contains("instruction text"),
            "英文提示要说明不要带方括号：{en}"
        );
        let zh = report.retry_hint(HintLang::Chinese).expect("应当有提示");
        assert!(zh.contains("方括号"), "中文提示要说明不要带方括号：{zh}");
    }
}
