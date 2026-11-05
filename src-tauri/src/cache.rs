//! 翻译缓存：内存态 + 磁盘持久化。
//!
//! ## 为什么需要落盘
//!
//! 批量翻译是最费时间的场景（300 段约 25 分钟），也是唯一会中断的场景
//! （引擎崩溃、用户取消、断电）。此前缓存纯内存，程序一关就清零，
//! 于是「翻到第 250 段崩了」→ 输出文件没写 → 全部重翻。
//!
//! 两件事配合才解决问题：
//! 1. 每翻完一段就追加写盘（本模块）
//! 2. 批量记录处理到第几段（batch.rs）
//!
//! ## 存储格式
//!
//! JSONL（每行一条 JSON），追加写：
//!
//! ```jsonl
//! {"k":"<缓存键>","t":"译文","f":false,"x":false}
//! {"k":"...","t":"...","f":true,"x":true}
//! ```
//!
//! 选择 JSONL 而不是 SQLite 的理由：
//! - 追加写是 O(1)，不需要事务、不需要锁文件
//! - 中途被杀最多丢掉最后半行，前面的行完好（**这正是我们要的**）
//! - 没有新依赖，项目已经依赖 serde_json
//!
//! ## 关键不变量（有测试锁住）
//!
//! 1. **被取消的片段绝不落盘** —— 否则用户会拿到一段看着正常、实际被截断的译文
//! 2. **损坏的行要跳过而不是整份丢弃** —— 半行 JSON 是正常情况，不能因此丢历史
//! 3. **术语表变了不能误命中** —— 缓存键已含术语，落盘不能绕过这一点
//! 4. **文件超过上限要压缩** —— 否则无限增长

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use crate::service::SegmentStats;

/// 落盘文件的最大行数。超过后压缩成「每个键只留最后一条」。
///
/// 为什么按行数而不是字节：一行的大小取决于译文长度，差异可达百倍；
/// 按行数更容易给出可预期的上限。换算过来约 4000 行 × 平均 200 字节 ≈ 800 KB。
pub const MAX_LINES: usize = 4000;

/// 一行缓存记录。
///
/// 字段名刻意用单字母：这个文件可能有几千行，省下的字节是实打实的。
/// `f` = flagged，`x` = fixed，与 [`SegmentStats`] 对应。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Record {
    /// 缓存键（含模型 / 语言对 / 术语 / 原文）
    pub k: String,
    /// 译文
    pub t: String,
    /// 质检是否检出问题
    #[serde(default)]
    pub f: bool,
    /// 检出的问题是否已被重试修好
    #[serde(default)]
    pub x: bool,
}

impl Record {
    pub fn new(key: &str, text: &str, stats: SegmentStats) -> Self {
        Self {
            k: key.to_string(),
            t: text.to_string(),
            f: stats.flagged,
            x: stats.fixed,
        }
    }

    pub fn stats(&self) -> SegmentStats {
        SegmentStats {
            flagged: self.f,
            fixed: self.x,
        }
    }
}

/// 磁盘缓存句柄。
///
/// 持有文件路径；每次写入都是「打开-追加-关闭」，不长期占用句柄。
/// 这样进程被杀时不会留下锁文件，也不需要处理「上次没正常关闭」。
pub struct DiskCache {
    path: PathBuf,
    /// 已经写过多少行（用于判断何时压缩）。启动时从文件行数初始化。
    lines: usize,
}

impl DiskCache {
    /// 打开（或创建）缓存文件，并把已有内容读进 `into`。
    ///
    /// 损坏的行会被跳过 —— 半行 JSON 是进程被杀的常见结果，
    /// 不能因此丢掉整个历史。
    pub fn open(path: PathBuf, into: &mut HashMap<String, (String, SegmentStats)>) -> Self {
        let mut lines = 0usize;
        if let Ok(file) = File::open(&path) {
            let reader = BufReader::new(file);
            for raw in reader.lines().map_while(Result::ok) {
                if raw.trim().is_empty() {
                    continue;
                }
                lines += 1;
                // 解析失败就跳过这一行：可能是被杀时写了一半
                if let Ok(rec) = serde_json::from_str::<Record>(&raw) {
                    // 后写的覆盖先写的：同一个键重翻过就以最新为准
                    let st = rec.stats();
                    into.insert(rec.k, (rec.t, st));
                }
            }
        }
        Self { path, lines }
    }

    /// 追加一条记录。失败不影响翻译主流程 —— 缓存是优化，不是正确性依赖。
    ///
    /// 返回是否真的写成功，便于测试断言；生产代码可以忽略返回值。
    pub fn append(&mut self, record: &Record) -> bool {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        else {
            return false;
        };
        let Ok(line) = serde_json::to_string(record) else {
            return false;
        };
        if writeln!(file, "{line}").is_err() {
            return false;
        }
        self.lines += 1;
        true
    }

    /// 行数是否已达上限。
    pub fn needs_compaction(&self) -> bool {
        self.lines >= MAX_LINES
    }

    /// 把文件压缩成「每个键只留最后一条」。
    ///
    /// 先写临时文件再原子替换：中途被杀时原文件仍完好，
    /// 不会出现「压缩到一半，缓存全没了」。
    pub fn compact(&mut self, map: &HashMap<String, (String, SegmentStats)>) -> bool {
        let tmp = self.path.with_extension("jsonl.tmp");

        let mut out = String::new();
        for (key, (text, stats)) in map {
            let rec = Record::new(key, text, *stats);
            match serde_json::to_string(&rec) {
                Ok(line) => {
                    out.push_str(&line);
                    out.push('\n');
                }
                Err(_) => continue,
            }
        }

        if fs::write(&tmp, out.as_bytes()).is_err() {
            let _ = fs::remove_file(&tmp);
            return false;
        }
        if fs::rename(&tmp, &self.path).is_err() {
            let _ = fs::remove_file(&tmp);
            return false;
        }
        self.lines = map.len();
        true
    }

    /// 清空缓存文件（用户主动清理 / 数据目录重置）。
    pub fn clear(&mut self) -> bool {
        self.lines = 0;
        match fs::remove_file(&self.path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(flagged: bool, fixed: bool) -> SegmentStats {
        SegmentStats { flagged, fixed }
    }

    /// 建一个临时目录，返回其中的缓存文件路径。
    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("xieyi-cache-test-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建临时目录");
        dir.join("translate.jsonl")
    }

    #[test]
    fn record_round_trips_through_json() {
        let rec = Record::new(
            "模型\u{1}en\u{1}zh\u{1}\u{1}hello",
            "你好",
            stats(true, false),
        );
        let line = serde_json::to_string(&rec).expect("序列化");
        let back: Record = serde_json::from_str(&line).expect("反序列化");
        assert_eq!(back, rec);
        assert_eq!(back.stats(), stats(true, false));
    }

    #[test]
    fn append_then_reopen_hits_after_restart() {
        // 这是本功能的核心承诺：程序重启后，同样的原文仍然命中
        let path = temp_path("restart");
        let key = "m\u{1}en\u{1}zh\u{1}\u{1}hello";

        {
            let mut disk = DiskCache::open(path.clone(), &mut HashMap::new());
            assert!(disk.append(&Record::new(key, "你好", stats(false, false))));
        }
        // 模拟重启：新的 map、新的 DiskCache
        let mut map = HashMap::new();
        let _disk = DiskCache::open(path.clone(), &mut map);

        let (text, _) = map.get(key).expect("重启后应该命中");
        assert_eq!(text, "你好");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn stats_survive_restart() {
        // 批量重复跑时「检出多少段/修好几段」不能因为重启而丢
        let path = temp_path("stats");
        let key = "k";
        {
            let mut disk = DiskCache::open(path.clone(), &mut HashMap::new());
            disk.append(&Record::new(key, "v", stats(true, true)));
        }
        let mut map = HashMap::new();
        DiskCache::open(path.clone(), &mut map);
        let (_, st) = map.get(key).expect("命中");
        assert!(st.flagged && st.fixed);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn corrupt_line_is_skipped_not_fatal() {
        // 进程被杀时最后一行可能是半截 JSON —— 不能因此丢掉整个历史
        let path = temp_path("corrupt");
        {
            let mut disk = DiskCache::open(path.clone(), &mut HashMap::new());
            disk.append(&Record::new("good1", "第一条", stats(false, false)));
            disk.append(&Record::new("good2", "第二条", stats(false, false)));
        }
        // 手动追加半行，模拟被杀
        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(f, "{{\"k\":\"half\",\"t\":\"半").unwrap();
        }

        let mut map = HashMap::new();
        DiskCache::open(path.clone(), &mut map);
        assert_eq!(map.len(), 2, "两条完整记录都要保留");
        assert!(map.contains_key("good1"));
        assert!(map.contains_key("good2"));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn later_record_overrides_earlier_for_same_key() {
        // 同一段重翻过，重启后应该拿到最新那次
        let path = temp_path("override");
        {
            let mut disk = DiskCache::open(path.clone(), &mut HashMap::new());
            disk.append(&Record::new("k", "旧译文", stats(true, false)));
            disk.append(&Record::new("k", "新译文", stats(false, false)));
        }
        let mut map = HashMap::new();
        DiskCache::open(path.clone(), &mut map);
        assert_eq!(map.get("k").unwrap().0, "新译文");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn different_glossary_does_not_collide() {
        // 缓存键含术语表，落盘不能绕过这一点：
        // 改了术语之后同样的原文必须视为不同条目
        let path = temp_path("glossary");
        let k1 = "m\u{1}en\u{1}zh\u{1}API=接口\u{1}rate limit";
        let k2 = "m\u{1}en\u{1}zh\u{1}API=应用程序接口\u{1}rate limit";
        {
            let mut disk = DiskCache::open(path.clone(), &mut HashMap::new());
            disk.append(&Record::new(k1, "接口速率限制", stats(false, false)));
            disk.append(&Record::new(
                k2,
                "应用程序接口速率限制",
                stats(false, false),
            ));
        }
        let mut map = HashMap::new();
        DiskCache::open(path.clone(), &mut map);
        assert_eq!(map.len(), 2, "术语不同必须是两条");
        assert_ne!(map.get(k1).unwrap().0, map.get(k2).unwrap().0);
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn needs_compaction_at_limit() {
        let path = temp_path("compact");
        let mut disk = DiskCache::open(path.clone(), &mut HashMap::new());
        assert!(!disk.needs_compaction());
        for i in 0..MAX_LINES {
            disk.append(&Record::new(&format!("k{i}"), "v", stats(false, false)));
        }
        assert!(disk.needs_compaction(), "到上限必须要求压缩");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn compact_keeps_one_record_per_key() {
        let path = temp_path("compact2");
        let mut disk = DiskCache::open(path.clone(), &mut HashMap::new());
        // 同一个键写三次，压缩后只该剩一条
        for text in ["v1", "v2", "v3"] {
            disk.append(&Record::new("k", text, stats(false, false)));
        }
        disk.append(&Record::new("other", "x", stats(false, false)));

        let mut map = HashMap::new();
        for text in ["v1", "v2", "v3"] {
            map.insert("k".to_string(), (text.to_string(), stats(false, false)));
        }
        map.insert("other".to_string(), ("x".to_string(), stats(false, false)));

        assert!(disk.compact(&map));
        assert!(!disk.needs_compaction());
        assert_eq!(disk.lines, 2);

        // 重新读一遍，应该只剩两条
        let mut after = HashMap::new();
        DiskCache::open(path.clone(), &mut after);
        assert_eq!(after.len(), 2);
        assert_eq!(after.get("k").unwrap().0, "v3");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn clear_removes_file_and_resets_counter() {
        let path = temp_path("clear");
        let mut disk = DiskCache::open(path.clone(), &mut HashMap::new());
        disk.append(&Record::new("k", "v", stats(false, false)));
        assert!(path.exists());
        assert!(disk.clear());
        assert!(!path.exists());
        assert_eq!(disk.lines, 0);
        // 再清一次也应该成功（幂等）
        assert!(disk.clear());
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
