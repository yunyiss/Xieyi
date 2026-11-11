//! 文件批量翻译：txt / md / srt 三种格式，翻译结果写到原文件旁边。
//!
//! 并发策略：**文件级并发 2**，文件内部串行。
//! 依据是本机实测（Hy-MT2-1.8B，4096 上下文）：
//! llama.cpp 的 KV 池在多个 slot 间共享，所以
//! 2 路并发（1200 字符分段 + 1024 max_tokens）稳定成功、约 1.9 倍加速；
//! 4 路并发则直接报 `Context size has been exceeded`。
//! 因此这里不做"文件内再并发"，避免叠加后打爆共享 KV。

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Instant;
use tauri::{AppHandle, Emitter};

use crate::service::{self, AppState, GlossaryEntry, SegmentStats, Service};
use crate::translate as segmenter;

/// 单个片段的字符上限，与翻译页保持一致。
const MAX_CHARS: usize = 1200;
/// 单个片段的尝试次数（含首次）。
const MAX_ATTEMPTS: usize = 3;
/// 同时处理的文件数。见文件头说明，不要随意调大。
const CONCURRENCY: usize = 2;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchRequest {
    pub model: String,
    pub source: String,
    pub target: String,
    /// 翻译风格（general/formal/technical/literary/subtitle），空值按 general。
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BatchProgress {
    path: String,
    index: usize,
    total: usize,
    /// reading | translating | writing | done | failed | skipped
    stage: String,
    /// 已完成的片段数
    done: usize,
    /// 总片段数（解析完成后才知道）
    segments: usize,
    failed_segments: usize,
    /// 这个文件有多少段是**从上次进度里恢复**的。
    /// 用于告诉用户「正在接着上次的翻」，而不是重头开始。
    #[serde(default)]
    resumed_segments: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BatchSummary {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    /// 所有文件里未能翻译、保留了原文的片段总数
    pub failed_segments: usize,
    /// 首次质检就检出问题的片段总数（含已修好的）
    pub quality_segments: usize,
    /// 其中重试后修好的片段数
    pub quality_fixed: usize,
    pub elapsed_ms: u64,
    pub cancelled: bool,
}

/// 单个文件的处理结果，用于最后汇总。
#[derive(Default)]
struct FileOutcome {
    ok: bool,
    skipped: bool,
    failed_segments: usize,
    flagged_segments: usize,
    fixed_segments: usize,
}

/// 待回填的组装结构。
enum Assembled {
    /// txt / md：`flags[i]` 表示第 i 段是否为段落结尾，决定拼接时加换行还是空格。
    Plain {
        bodies: Vec<String>,
        flags: Vec<bool>,
    },
    /// srt：head 为序号 + 时间轴（原样保留），bodies 为字幕正文。
    Srt {
        heads: Vec<Vec<String>>,
        bodies: Vec<String>,
    },
}

#[tauri::command]
pub async fn batch_translate(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    request: BatchRequest,
) -> Result<BatchSummary, String> {
    if request.paths.is_empty() {
        return Err("没有待翻译的文件".into());
    }

    let started = Instant::now();
    let service = service::ensure(&app, &state).await?;
    let glossary = service::load_glossary(&app);

    // 批处理期间独占推理通道，避免和翻译页 / 聊天的请求互相插队
    let _infer = state.infer_lock.lock().await;
    // 批量可能跑很久，期间不能算"空闲"，否则 TTL 会把正在用的模型卸掉
    let _busy = service::InflightGuard::new(&state.inflight);
    state.cancel.store(false, Ordering::Relaxed);

    // 先把模型装好，别让第一个文件独自承担加载时间
    service.load(&app, &request.model, &state.cancel).await?;

    let total = request.paths.len();
    let outcomes = tokio::sync::Mutex::new(Vec::<FileOutcome>::new());

    stream::iter(request.paths.iter().enumerate())
        .for_each_concurrent(CONCURRENCY, |(index, path)| {
            let outcomes = &outcomes;
            let service = &service;
            let glossary = &glossary;
            let request = &request;
            let state = &state;
            let app = &app;

            async move {
                let outcome =
                    process_file(app, state, service, request, glossary, path, index, total).await;
                outcomes.lock().await.push(outcome);
            }
        })
        .await;

    *state.last_used.lock().await = Instant::now();

    let outcomes = outcomes.into_inner();
    let cancelled = state.cancel.load(Ordering::Relaxed);

    Ok(BatchSummary {
        total,
        succeeded: outcomes.iter().filter(|o| o.ok).count(),
        failed: outcomes.iter().filter(|o| !o.ok && !o.skipped).count(),
        skipped: outcomes.iter().filter(|o| o.skipped).count(),
        failed_segments: outcomes.iter().map(|o| o.failed_segments).sum(),
        quality_segments: outcomes.iter().map(|o| o.flagged_segments).sum(),
        quality_fixed: outcomes.iter().map(|o| o.fixed_segments).sum(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        cancelled,
    })
}

/// 中止批量任务：只置取消位，正在跑的片段会在下一个检查点退出。
#[tauri::command]
pub fn batch_cancel(state: tauri::State<'_, AppState>) {
    state.cancel.store(true, Ordering::Relaxed);
}

#[allow(clippy::too_many_arguments)]
async fn process_file(
    app: &AppHandle,
    state: &AppState,
    service: &Service,
    request: &BatchRequest,
    glossary: &[GlossaryEntry],
    path: &str,
    index: usize,
    total: usize,
) -> FileOutcome {
    // 从上次进度里恢复的片段数。在读取源文件后才知道真值，
    // 先给 0（此时进度事件还没有 segments 信息）。
    let mut resumed_count = 0usize;

    macro_rules! report {
        ($stage:expr, $done:expr, $segments:expr, $failed:expr, $out:expr, $err:expr) => {
            let _ = app.emit(
                "batch://progress",
                BatchProgress {
                    path: path.to_string(),
                    index,
                    total,
                    stage: $stage.to_string(),
                    done: $done,
                    segments: $segments,
                    failed_segments: $failed,
                    resumed_segments: resumed_count,
                    output: $out,
                    error: $err,
                },
            );
        };
    }

    report!("reading", 0, 0, 0, None, None);

    if state.cancel.load(Ordering::Relaxed) {
        report!("skipped", 0, 0, 0, None, Some("已取消".into()));
        return FileOutcome {
            skipped: true,
            ..Default::default()
        };
    }

    let source = PathBuf::from(path);

    let Some(output) = output_path(&source, &request.target) else {
        report!("failed", 0, 0, 0, None, Some("无法推导输出文件名".into()));
        return FileOutcome::default();
    };

    // 不覆盖已有译文，避免重跑一次就把上次的结果冲掉
    if output.exists() {
        report!(
            "skipped",
            0,
            0,
            0,
            Some(output.to_string_lossy().to_string()),
            Some("输出文件已存在".into())
        );
        return FileOutcome {
            skipped: true,
            ..Default::default()
        };
    }

    let bytes = match std::fs::read(&source) {
        Ok(b) => b,
        Err(e) => {
            report!("failed", 0, 0, 0, None, Some(format!("读取失败: {e}")));
            return FileOutcome::default();
        }
    };

    let (units, mut assembled) = split(&decode(&bytes), &source);

    if units.is_empty() {
        report!("failed", 0, 0, 0, None, Some("没有可翻译的文本".into()));
        return FileOutcome::default();
    }

    let segments = units.len();
    report!("translating", 0, segments, 0, None, None);

    let mut failed = 0usize;
    let mut flagged = 0usize;
    let mut fixed = 0usize;

    // ── 断点续传 ────────────────────────────────────────────────────
    //
    // 每翻完一段就把结果写进旁路进度文件。中断后重新拖入同一个文件，
    // 已完成的段直接从进度文件回填，不再请求模型。
    //
    // 为什么不能只靠翻译缓存：失败片段（模型返回空）不进缓存，
    // 且缓存有 4000 条上限、会被压缩，长文档跨天续跑时不保证还在。
    let partial = crate::partial::partial_path(&output);
    let fp = crate::partial::fingerprint(&source).unwrap_or_default();
    let resumed = crate::partial::load(&partial, &fp, &request.target, segments);
    resumed_count = resumed.len();

    // 已有的进度直接回填到装配结构里：SRT 的序号/时间轴保持不变
    for (i, text) in &resumed {
        set_body(&mut assembled, *i, text.clone());
    }
    let mut done = resumed.len();
    if !resumed.is_empty() {
        report!("translating", done, segments, 0, None, None);
    }

    // 续传时沿用旧进度文件；全新任务则重建一份（覆盖可能残留的旧文件）
    if resumed.is_empty() {
        let _ = crate::partial::remove(&partial);
        let _ = crate::partial::create(
            &partial,
            &crate::partial::Meta {
                fingerprint: fp.clone(),
                target: request.target.clone(),
                segments,
            },
        );
    }

    for (i, text) in units.iter().enumerate() {
        if state.cancel.load(Ordering::Relaxed) {
            break;
        }
        // 这一段上次已经翻好了，跳过（不回填，前面已经处理过）
        if resumed.contains_key(&i) {
            continue;
        }

        match translate_with_retry(service, app, state, request, glossary, text).await {
            Some((translated, stats)) => {
                if stats.flagged {
                    flagged += 1;
                }
                if stats.fixed {
                    fixed += 1;
                }
                set_body(&mut assembled, i, translated.clone());
                // 立刻落盘，不等整个文件翻完。
                // 写失败不影响翻译（进度是优化，不是正确性依赖）。
                let _ =
                    crate::partial::append(&partial, &crate::partial::Entry { i, t: translated });
                done += 1;
            }
            // 保留原文：宁可混一段原文，也不要静默丢内容。
            // 失败的不写进度 —— 下次续传会重试这一段。
            None => {
                failed += 1;
                set_body(&mut assembled, i, text.clone());
            }
        }

        report!("translating", done, segments, failed, None, None);
    }

    if state.cancel.load(Ordering::Relaxed) {
        // **保留进度文件**：这是断点续传的关键 —— 下次拖入同一个文件
        // 能接着翻。删掉就等于强制从头再来。
        report!(
            "skipped",
            done,
            segments,
            failed,
            None,
            Some("已取消".into())
        );
        return FileOutcome {
            skipped: true,
            failed_segments: failed,
            flagged_segments: flagged,
            fixed_segments: fixed,
            ..Default::default()
        };
    }

    report!("writing", done, segments, failed, None, None);

    let rendered = render(&assembled, &request.target);

    if let Err(e) = std::fs::write(&output, rendered.as_bytes()) {
        report!(
            "failed",
            done,
            segments,
            failed,
            None,
            Some(format!("写入失败: {e}"))
        );
        return FileOutcome {
            failed_segments: failed,
            flagged_segments: flagged,
            fixed_segments: fixed,
            ..Default::default()
        };
    }

    // 全部翻完并写出成品：进度文件没用了，删掉不给用户留垃圾
    let _ = crate::partial::remove(&partial);

    report!(
        "done",
        done,
        segments,
        failed,
        Some(output.to_string_lossy().to_string()),
        None
    );
    FileOutcome {
        ok: true,
        failed_segments: failed,
        flagged_segments: flagged,
        fixed_segments: fixed,
        ..Default::default()
    }
}

/// 失败重试：最多 `MAX_ATTEMPTS` 次，全部失败返回 `None`。
async fn translate_with_retry(
    service: &Service,
    app: &AppHandle,
    state: &AppState,
    request: &BatchRequest,
    glossary: &[GlossaryEntry],
    text: &str,
) -> Option<(String, SegmentStats)> {
    let mut last_error = String::new();

    for attempt in 0..MAX_ATTEMPTS {
        if state.cancel.load(Ordering::Relaxed) {
            return None;
        }
        if attempt > 0 {
            // 退避一下：多半是并发把共享 KV 池挤满了
            tokio::time::sleep(std::time::Duration::from_millis(400 * attempt as u64)).await;
        }

        match service
            .translate_segment(
                app,
                state,
                None,
                &request.model,
                &request.source,
                &request.target,
                glossary,
                text,
                // 批量是落盘唯一的真实场景：耗时长、会中断，重来代价高。
                // 翻译页的临时译文不该写盘。
                crate::service::CacheMode::Persistent,
                &state.cancel,
            )
            .await
        {
            Ok((translated, stats, _from_cache)) if !translated.trim().is_empty() => {
                return Some((translated, stats));
            }
            Ok(_) => last_error = "模型返回空内容".into(),
            Err(e) => last_error = e,
        }
    }

    eprintln!("批量翻译片段失败（已重试 {MAX_ATTEMPTS} 次）: {last_error}");
    None
}

/// 解码文件内容：优先 UTF-8（含 BOM），失败则按 GB18030 解（中文 srt 常见）。
fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let (text, _, _) = encoding_rs::GB18030.decode(bytes);
            text.into_owned()
        }
    }
}

/// 输出路径：`foo.srt` → `foo.zh.srt`。
fn output_path(input: &Path, target: &str) -> Option<PathBuf> {
    let stem = input.file_stem()?;
    let ext = input.extension()?;

    let mut name = stem.to_os_string();
    name.push(format!(".{target}."));
    name.push(ext);

    Some(input.with_file_name(name))
}

/// 把原文切成待翻译单元，并准备好回填结构。
/// 返回的单元与组装结构一一对应，`set_body(i, ..)` 即可回填第 i 段。
fn split(content: &str, path: &Path) -> (Vec<String>, Assembled) {
    let is_srt = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("srt"))
        .unwrap_or(false);

    if is_srt {
        // 字幕按块翻；解析不出时间轴就说明不是合法 srt，退回纯文本路径
        if let Some(blocks) = parse_srt(content) {
            let heads = blocks.iter().map(|b| b.head.clone()).collect();
            let bodies: Vec<String> = blocks.iter().map(|b| b.text.clone()).collect();
            return (bodies.clone(), Assembled::Srt { heads, bodies });
        }
    }

    let segments = segmenter::split_segments(content, MAX_CHARS);
    let bodies: Vec<String> = segments.iter().map(|s| s.text.clone()).collect();
    let flags: Vec<bool> = segments.iter().map(|s| s.paragraph).collect();

    let units = bodies.clone();
    (units, Assembled::Plain { bodies, flags })
}

/// 回填第 `index` 段的译文。
fn set_body(assembled: &mut Assembled, index: usize, text: String) {
    match assembled {
        Assembled::Plain { bodies, .. } => {
            if index < bodies.len() {
                bodies[index] = text;
            }
        }
        Assembled::Srt { bodies, .. } => {
            if index < bodies.len() {
                bodies[index] = text;
            }
        }
    }
}

/// 把译文拼回文件内容。
fn render(assembled: &Assembled, target: &str) -> String {
    match assembled {
        Assembled::Plain { bodies, flags } => {
            let mut out = String::new();
            for (i, body) in bodies.iter().enumerate() {
                out.push_str(body);
                if i + 1 < bodies.len() {
                    out.push_str(segmenter::joiner(flags[i], target));
                }
            }
            out
        }
        Assembled::Srt { heads, bodies } => {
            let mut out = String::new();
            for (i, head) in heads.iter().enumerate() {
                out.push_str(&head.join("\n"));
                out.push('\n');
                if let Some(body) = bodies.get(i) {
                    out.push_str(body.trim());
                }
                out.push_str("\n\n");
            }
            out
        }
    }
}

struct SrtBlock {
    /// 序号行与时间轴行，原样保留
    head: Vec<String>,
    /// 正文。多行合并成一行——模型并不会可靠地保留换行，与其赌不如定死
    text: String,
}

/// 解析 srt。任何一块缺少时间轴就判定为"不是 srt"，返回 `None`。
fn parse_srt(content: &str) -> Option<Vec<SrtBlock>> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let mut blocks = Vec::new();

    for raw in normalized.split("\n\n") {
        let block = raw.trim_matches('\n');
        if block.trim().is_empty() {
            continue;
        }

        let lines: Vec<&str> = block.lines().collect();
        let arrow = lines.iter().position(|l| l.contains("-->"))?;

        let head: Vec<String> = lines[..=arrow]
            .iter()
            .map(|l| l.trim().to_string())
            .collect();
        let text = lines[arrow + 1..]
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");

        if text.is_empty() {
            continue;
        }

        blocks.push(SrtBlock { head, text });
    }

    if blocks.is_empty() {
        None
    } else {
        Some(blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_keeps_index_and_timestamp() {
        let src = "1\n00:00:01,000 --> 00:00:03,500\nHello world\n\n2\n00:00:04,000 --> 00:00:06,000\nSecond line\nwrapped here\n";
        let blocks = parse_srt(src).expect("应能解析");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].head, vec!["1", "00:00:01,000 --> 00:00:03,500"]);
        assert_eq!(blocks[0].text, "Hello world");
        // 多行字幕合并成一行，时间轴不变
        assert_eq!(blocks[1].text, "Second line wrapped here");
    }

    #[test]
    fn srt_render_round_trips_structure() {
        let src =
            "1\n00:00:01,000 --> 00:00:03,500\nHello\n\n2\n00:00:04,000 --> 00:00:06,000\nWorld\n";
        let blocks = parse_srt(src).unwrap();
        let heads: Vec<Vec<String>> = blocks.iter().map(|b| b.head.clone()).collect();
        let bodies = vec!["你好".to_string(), "世界".to_string()];
        let out = render(&Assembled::Srt { heads, bodies }, "zh");
        assert_eq!(
            out,
            "1\n00:00:01,000 --> 00:00:03,500\n你好\n\n2\n00:00:04,000 --> 00:00:06,000\n世界\n\n"
        );
    }

    // -- 断点续传：与切分/回填的协同 --

    #[test]
    fn partial_progress_aligns_with_units() {
        // 进度里的索引必须就是 units 的下标。
        // 这条锁住「两边用同一套切分」这个前提 ——
        // 一旦切分规则变了，续传会串位。
        let content = "第一段

第二段

第三段";
        let (units, mut assembled) = split(content, Path::new("a.md"));
        assert_eq!(units.len(), 3);

        // 模拟：只完成了第 0 和第 2 段
        let mut done = std::collections::BTreeMap::new();
        done.insert(0usize, "译文一".to_string());
        done.insert(2usize, "译文三".to_string());

        for (i, text) in &done {
            set_body(&mut assembled, *i, text.clone());
        }
        let rendered = render(&assembled, "zh");
        assert!(rendered.contains("译文一"));
        assert!(rendered.contains("译文三"));
        // 未完成的那段保留原文（不能出现空段）
        assert!(rendered.contains("第二段"));
    }

    #[test]
    fn srt_resume_preserves_index_and_timeline() {
        // SRT 续传时，序号与时间轴必须原样保留
        let srt =
            "1\n00:00:01,000 --> 00:00:03,500\nHello\n\n2\n00:00:04,000 --> 00:00:06,000\nWorld\n";
        let (units, mut assembled) = split(srt, Path::new("m.srt"));
        assert_eq!(units.len(), 2);

        // 只续上第二段
        let mut done = std::collections::BTreeMap::new();
        done.insert(1usize, "世界".to_string());
        for (i, text) in &done {
            set_body(&mut assembled, *i, text.clone());
        }

        let rendered = render(&assembled, "zh");
        assert!(rendered.contains("00:00:01,000 --> 00:00:03,500"));
        assert!(rendered.contains("00:00:04,000 --> 00:00:06,000"));
        assert!(rendered.contains("世界"));
        // 未完成的保留原文
        assert!(rendered.contains("Hello"));
    }

    #[test]
    fn plain_text_units_align_with_assembled_bodies() {
        let src = "第一段。\n\n第二段。";
        let (units, assembled) = split(src, Path::new("a.txt"));
        assert_eq!(units, vec!["第一段。", "第二段。"]);

        let Assembled::Plain { bodies, flags } = &assembled else {
            panic!("应为纯文本结构");
        };
        // 单元、正文、段落标记三者下标必须一一对应，否则回填会错位
        assert_eq!(bodies, &units);
        assert_eq!(flags, &vec![true, true]);
    }

    #[test]
    fn plain_render_uses_blank_line_between_paragraphs() {
        let bodies = vec!["一。".to_string(), "二。".to_string()];
        let flags = vec![false, true];
        let out = render(&Assembled::Plain { bodies, flags }, "zh");
        assert_eq!(out, "一。二。");
    }

    #[test]
    fn broken_srt_falls_back_to_plain_text() {
        // 没有时间轴，应退回纯文本而不是当成字幕
        let (units, assembled) = split("就是一段普通文字\n\n第二段", Path::new("a.srt"));
        assert_eq!(units.len(), 2);
        assert!(matches!(assembled, Assembled::Plain { .. }));
    }

    #[test]
    fn output_name_keeps_extension_and_adds_target() {
        // 用 Path 比较，避免 Windows 的 `\` 与输入里的 `/` 造成假的失败
        let out = output_path(Path::new("docs/report.md"), "zh").unwrap();
        assert_eq!(out, Path::new("docs/report.zh.md"));
        assert_eq!(out.extension().unwrap(), "md");

        let out = output_path(Path::new("subs/movie.srt"), "ja").unwrap();
        assert_eq!(out, Path::new("subs/movie.ja.srt"));
    }

    #[test]
    fn output_path_rejects_files_without_extension() {
        assert!(output_path(Path::new("subs/README"), "zh").is_none());
    }

    #[test]
    fn utf8_bom_is_stripped() {
        assert_eq!(decode(b"\xEF\xBB\xBFhello"), "hello");
    }

    #[test]
    fn gb18030_content_is_decoded() {
        // "中文" 的 GB18030 编码
        assert_eq!(decode(&[0xD6, 0xD0, 0xCE, 0xC4]), "中文");
    }
}
