//! 批量任务的断点续传：把已完成片段持久化到旁路文件。
//!
//! ## 为什么需要
//!
//! 批量是最费时间的场景（300 段约 25 分钟），也是唯一会中断的场景
//! （引擎崩溃、用户取消、断电）。而现有流程是「全部翻完才写输出文件」，
//! 所以中断意味着**输出文件不存在 → 下次从头再来**。
//!
//! 翻译缓存解决了「已翻过的段落重跑很快」，但解决不了两件事：
//! 1. 失败片段不进缓存（模型返回空），重跑仍要重新请求
//! 2. 段落切分、SRT 结构这些解析结果每次都要重算
//!
//! 更关键的是：缓存有 4000 条上限、会被压缩，长文档跨天续跑时不一定还在。
//!
//! ## 设计
//!
//! 每个输入文件配一个旁路进度文件：`foo.md` → `.foo.zh.md.partial`。
//!
//! - **JSONL 追加写**：每完成一段追加一行 `{"i":3,"t":"..."}`。
//!   追加是 O(1)，中途被杀最多丢最后半行，前面的行完好。
//! - **指纹校验**：记录源文件的大小 + 修改时间 + 目标语言。
//!   任一不匹配就放弃续传 —— 源文件改过了，接着翻旧进度会错位。
//! - **完成即删**：全部翻完写出最终文件后删除进度文件，
//!   不给用户留垃圾。
//!
//! ## 为什么不直接写输出文件
//!
//! 边翻边写输出文件会让「半成品」看起来像「成品」，用户容易误以为
//! 翻完了。旁路文件明确表示「这是中间状态」。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// 进度文件后缀。以点开头，避免和真正的输出文件混淆。
pub const PARTIAL_SUFFIX: &str = ".partial";

/// 一行进度记录。字段名用单字母：文件可能上千行。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Entry {
    /// 片段索引（从 0 开始，对应 `split()` 出来的 units 下标）
    pub i: usize,
    /// 该片段的译文
    pub t: String,
}

/// 进度文件头，记录「这份进度对应哪个源文件」。
///
/// 单独一行放在文件开头（`{"meta":{...}}` 形式），读取时先解析它。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Meta {
    /// 源文件指纹：大小 + 修改时间（秒）
    pub fingerprint: String,
    /// 目标语言。换语言不能续传
    pub target: String,
    /// 片段总数。变了说明切分规则或源文件变了
    pub segments: usize,
}

/// 进度文件的一行：要么是头，要么是一条已完成记录。
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
enum Line {
    Meta { meta: Meta },
    Entry { i: usize, t: String },
}

/// 算出某个输出文件对应的进度文件路径。
///
/// `foo.md` + `zh` → `foo.zh.md` → 旁边放 `.foo.zh.md.partial`
pub fn partial_path(output: &Path) -> PathBuf {
    let name = output
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let hidden = format!(".{name}{PARTIAL_SUFFIX}");
    match output.parent() {
        Some(dir) => dir.join(hidden),
        None => PathBuf::from(hidden),
    }
}

/// 源文件指纹：大小 + 修改时间。
///
/// 不用内容哈希：大文件算哈希要几百毫秒，而「大小 + mtime」对
/// 「用户改了这个文件」这个判断已经足够 —— 真改内容几乎必然改 mtime。
pub fn fingerprint(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let len = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(format!("{len}:{mtime}"))
}

/// 读取已有的进度。返回 `(已完成片段, 记录的总段数)`。
///
/// 任何一个条件不满足都返回空 —— **宁可从头翻，也不要错位拼接**：
/// - 文件不存在 / 打不开
/// - 头部损坏或缺失
/// - 指纹、目标语言、片段总数任一不匹配
/// - 单行损坏（跳过那一行，其余照收）
pub fn load(
    path: &Path,
    fingerprint: &str,
    target: &str,
    segments: usize,
) -> BTreeMap<usize, String> {
    let mut out = BTreeMap::new();
    let Ok(file) = File::open(path) else {
        return out;
    };

    let mut expected: Option<Meta> = None;
    for raw in BufReader::new(file).lines().map_while(Result::ok) {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        // 半行 JSON 会被解析失败，跳过即可
        let Ok(line) = serde_json::from_str::<Line>(raw) else {
            continue;
        };
        match line {
            Line::Meta { meta } => {
                // 头部校验：三个条件全中才认这份进度
                if meta.fingerprint != fingerprint
                    || meta.target != target
                    || meta.segments != segments
                {
                    return BTreeMap::new();
                }
                expected = Some(meta);
            }
            Line::Entry { i, t } => {
                // 头部还没读到就先跳过（正常文件头在第一行）
                if expected.is_none() {
                    continue;
                }
                if i < segments {
                    out.insert(i, t);
                }
            }
        }
    }
    out
}

/// 开一份新进度文件（覆盖旧的），写入头部。返回是否成功。
pub fn create(path: &Path, meta: &Meta) -> bool {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let line = match serde_json::to_string(&Line::Meta { meta: meta.clone() }) {
        Ok(l) => l,
        Err(_) => return false,
    };
    fs::write(path, format!("{line}\n")).is_ok()
}

/// 追加一条已完成记录。失败不影响翻译主流程 —— 进度是优化，不是正确性依赖。
pub fn append(path: &Path, entry: &Entry) -> bool {
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return false;
    };
    let Ok(line) = serde_json::to_string(&Line::Entry {
        i: entry.i,
        t: entry.t.clone(),
    }) else {
        return false;
    };
    writeln!(file, "{line}").is_ok()
}

/// 删除进度文件（正常完成或用户主动放弃时调用）。
pub fn remove(path: &Path) -> bool {
    match fs::remove_file(path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("xieyi-partial-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn meta(fp: &str, target: &str, segments: usize) -> Meta {
        Meta {
            fingerprint: fp.to_string(),
            target: target.to_string(),
            segments,
        }
    }

    #[test]
    fn partial_path_sits_next_to_output_and_is_hidden() {
        let out = Path::new("/tmp/docs/report.zh.md");
        let p = partial_path(out);
        assert_eq!(p, Path::new("/tmp/docs/.report.zh.md.partial"));
        // 点开头：与真正的输出文件区分开
        assert!(p.file_name().unwrap().to_string_lossy().starts_with('.'));
    }

    #[test]
    fn write_then_resume_restores_completed_segments() {
        // 核心承诺：中断后重入，已完成的段不用再翻
        let dir = tmp("resume");
        let p = dir.join(".x.partial");
        let m = meta("100:1234", "zh", 3);

        assert!(create(&p, &m));
        assert!(append(
            &p,
            &Entry {
                i: 0,
                t: "第一段".into()
            }
        ));
        assert!(append(
            &p,
            &Entry {
                i: 2,
                t: "第三段".into()
            }
        ));

        let done = load(&p, "100:1234", "zh", 3);
        assert_eq!(done.len(), 2);
        assert_eq!(done.get(&0).unwrap(), "第一段");
        assert_eq!(done.get(&2).unwrap(), "第三段");
        // 第 1 段没翻过，要保持缺失（调用方据此判断要翻哪些）
        assert!(!done.contains_key(&1));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn changed_fingerprint_discards_progress() {
        // 源文件被改过：接着翻旧进度会错位，必须从头来
        let dir = tmp("fingerprint");
        let p = dir.join(".x.partial");
        assert!(create(&p, &meta("100:1234", "zh", 3)));
        assert!(append(
            &p,
            &Entry {
                i: 0,
                t: "旧译文".into()
            }
        ));

        let done = load(&p, "200:9999", "zh", 3);
        assert!(done.is_empty(), "指纹变了必须丢弃进度");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn changed_target_language_discards_progress() {
        let dir = tmp("target");
        let p = dir.join(".x.partial");
        assert!(create(&p, &meta("100:1234", "zh", 3)));
        assert!(append(
            &p,
            &Entry {
                i: 0,
                t: "中文".into()
            }
        ));

        assert!(load(&p, "100:1234", "ja", 3).is_empty(), "换语言必须丢弃");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn changed_segment_count_discards_progress() {
        // 切分结果变了（比如改了分段上限），索引对不上，必须丢弃
        let dir = tmp("segments");
        let p = dir.join(".x.partial");
        assert!(create(&p, &meta("100:1234", "zh", 3)));
        assert!(append(
            &p,
            &Entry {
                i: 0,
                t: "a".into()
            }
        ));

        assert!(load(&p, "100:1234", "zh", 5).is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_line_is_skipped_not_fatal() {
        // 进程被杀时最后一行可能是半截 JSON
        let dir = tmp("corrupt");
        let p = dir.join(".x.partial");
        assert!(create(&p, &meta("100:1234", "zh", 4)));
        assert!(append(
            &p,
            &Entry {
                i: 0,
                t: "完整".into()
            }
        ));
        {
            let mut f = OpenOptions::new().append(true).open(&p).unwrap();
            writeln!(f, "{{\"i\":1,\"t\":\"半").unwrap();
        }
        assert!(append(
            &p,
            &Entry {
                i: 2,
                t: "后续".into()
            }
        ));

        let done = load(&p, "100:1234", "zh", 4);
        assert_eq!(done.len(), 2, "两条完整的都要保留");
        assert!(done.contains_key(&0) && done.contains_key(&2));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_file_yields_empty() {
        let dir = tmp("missing");
        let p = dir.join(".nope.partial");
        assert!(load(&p, "1:2", "zh", 3).is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn remove_is_idempotent() {
        let dir = tmp("remove");
        let p = dir.join(".x.partial");
        assert!(create(&p, &meta("1:2", "zh", 3)));
        assert!(p.exists());
        assert!(remove(&p));
        assert!(!p.exists());
        // 再删一次也算成功
        assert!(remove(&p));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn later_entry_overrides_earlier_for_same_index() {
        // 同一段重试成功后，应该以后写的为准
        let dir = tmp("override");
        let p = dir.join(".x.partial");
        assert!(create(&p, &meta("1:2", "zh", 2)));
        assert!(append(
            &p,
            &Entry {
                i: 0,
                t: "旧".into()
            }
        ));
        assert!(append(
            &p,
            &Entry {
                i: 0,
                t: "新".into()
            }
        ));

        let done = load(&p, "1:2", "zh", 2);
        assert_eq!(done.get(&0).unwrap(), "新");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn out_of_range_index_is_ignored() {
        // 防御：文件被手改出越界索引时不能污染结果
        let dir = tmp("range");
        let p = dir.join(".x.partial");
        assert!(create(&p, &meta("1:2", "zh", 2)));
        assert!(append(
            &p,
            &Entry {
                i: 99,
                t: "越界".into()
            }
        ));

        assert!(load(&p, "1:2", "zh", 2).is_empty());
        let _ = fs::remove_dir_all(dir);
    }
}
