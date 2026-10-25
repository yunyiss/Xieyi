/// 一个待翻译片段。`paragraph` 表示它是某个段落的最后一段，
/// 拼接时后面应接换行而不是空格。
pub struct Segment {
    pub text: String,
    pub paragraph: bool,
}

/// 把长文本切成适合单次推理的片段。
///
/// 规则：
/// 1. 先按空行切段落；
/// 2. 段落超过 `max_chars` 时按句末标点再切，并按上限打包；
/// 3. 每个段落最后一片标记 `paragraph = true`，用来还原空行结构。
pub fn split_segments(input: &str, max_chars: usize) -> Vec<Segment> {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = Vec::new();

    for paragraph in normalized.split("\n\n") {
        let text = paragraph.trim();
        if text.is_empty() {
            continue;
        }

        if text.chars().count() <= max_chars {
            out.push(Segment {
                text: text.to_string(),
                paragraph: true,
            });
            continue;
        }

        let sentences = split_sentences(text);
        let mut buffer = String::new();

        for sentence in sentences {
            if !buffer.is_empty()
                && buffer.chars().count() + sentence.chars().count() > max_chars
            {
                out.push(Segment {
                    text: std::mem::take(&mut buffer),
                    paragraph: false,
                });
            }
            buffer.push_str(&sentence);
        }

        if !buffer.is_empty() {
            out.push(Segment {
                text: buffer,
                paragraph: true,
            });
        }
    }

    out
}

fn is_terminator(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '!' | '?' | '…' | '；' | ';')
}

/// 按句末标点切句，保留标点本身。
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        current.push(c);

        let ends = is_terminator(c)
            || (c == '.'
                && chars
                    .peek()
                    .map(|n| n.is_whitespace() || *n == '\n')
                    .unwrap_or(true));

        if ends {
            // 吸收紧跟的引用/括号，避免把标点留在下一句开头
            while let Some(&n) = chars.peek() {
                if matches!(n, '”' | '’' | '"' | '\'' | '」' | '』' | '）' | ')' | '】' | ']') {
                    current.push(n);
                    chars.next();
                } else {
                    break;
                }
            }
            out.push(std::mem::take(&mut current));
        }
    }

    if !current.trim().is_empty() {
        out.push(current);
    }

    if out.is_empty() {
        out.push(text.to_string());
    }

    out
}

/// 片段之间的拼接符：段落结束换行，句中断句按目标语言决定加不加空格。
pub fn joiner(paragraph: bool, target: &str) -> &'static str {
    if paragraph {
        "\n\n"
    } else if matches!(target, "zh" | "ja" | "ko") {
        ""
    } else {
        " "
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_single_paragraph() {
        let segments = split_segments("Hello world.", 1200);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello world.");
        assert!(segments[0].paragraph);
    }

    #[test]
    fn paragraphs_are_split_and_flagged() {
        let segments = split_segments("第一段。\n\n第二段。\n\n第三段。", 1200);
        assert_eq!(segments.len(), 3);
        assert!(segments.iter().all(|s| s.paragraph));
        assert_eq!(segments[1].text, "第二段。");
    }

    #[test]
    fn long_paragraph_is_packed_within_limit() {
        let sentence = "这是一个用来测试分段的句子。";
        let text = sentence.repeat(40);
        let segments = split_segments(&text, 100);
        assert!(segments.len() > 1);
        assert!(segments.iter().all(|s| s.text.chars().count() <= 100));
        // 同一段落内只有最后一片标记为段落结束
        assert!(segments.last().unwrap().paragraph);
        assert!(!segments[0].paragraph);
    }

    #[test]
    fn english_sentences_split_on_period() {
        let segments = split_segments("One. Two. Three.", 8);
        assert!(segments.len() >= 2);
        assert!(segments
            .iter()
            .all(|s| s.text.chars().count() <= 8));
    }

    #[test]
    fn joiner_respects_target_language() {
        assert_eq!(joiner(true, "zh"), "\n\n");
        assert_eq!(joiner(false, "zh"), "");
        assert_eq!(joiner(false, "en"), " ");
    }

    #[test]
    fn empty_input_yields_no_segments() {
        assert!(split_segments("   \n\n  ", 1200).is_empty());
    }
}
