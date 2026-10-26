use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tauri::AppHandle;

use crate::hardware;
use crate::params;
use crate::paths;

/// 表示"自动按显存分配层数"的哨兵值。
pub const AUTO_LAYERS: u64 = 1001;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelFile {
    pub id: String,
    pub path: String,
    pub size_mb: u64,
}

/// 扫描模型目录下的 GGUF 文件（最多两层子目录）。
pub fn scan(app: &AppHandle) -> Vec<ModelFile> {
    let dir = match paths::models_dir(app) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let _ = paths::ensure(&dir);
    let mut out = Vec::new();
    collect(&dir, 0, &mut out);
    out.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
    out
}

fn collect(dir: &Path, depth: u8, out: &mut Vec<ModelFile>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < 1 {
                collect(&path, depth + 1, out);
            }
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("gguf") {
            continue;
        }

        // mmproj 是视觉项目的投影文件，不能单独作为模型加载
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.to_lowercase().contains("mmproj") {
            continue;
        }

        let size_mb = std::fs::metadata(&path)
            .map(|m| (m.len() as f64 / 1_048_576.0).round() as u64)
            .unwrap_or(0);

        out.push(ModelFile {
            id: stem(&path),
            path: path.to_string_lossy().to_string(),
            size_mb,
        });
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

/// 生成路由模式用的 preset。
/// 上下文按模型取用户配置（默认 4096），避免默认 262144 吃爆显存。
/// `gpu_layers` 为 [`AUTO_LAYERS`] 时按模型体积和当前空闲显存自动估算。
pub fn write_preset(app: &AppHandle, models: &[ModelFile], gpu_layers: u64) -> Result<PathBuf, String> {
    let root = paths::runtime_root(app)?;
    paths::ensure(&root)?;
    let path = root.join("presets.ini");

    let free_vram = if gpu_layers == AUTO_LAYERS {
        hardware::free_vram_mb()
    } else {
        None
    };

    let mut ini = String::new();
    for m in models {
        let layers = if gpu_layers == AUTO_LAYERS {
            let layer_count = read_layer_count(Path::new(&m.path));
            free_vram
                .map(|free| estimate_layers(m.size_mb, layer_count, free))
                .unwrap_or(999)
        } else {
            gpu_layers
        };

        let ctx = params::ctx_of(app, &m.id);

        ini.push_str(&format!(
            "[{}]\nmodel = {}\njinja = 1\nctx-size = {}\nn-gpu-layers = {}\ncache-type-k = q8_0\ncache-type-v = q8_0\n\n",
            m.id, m.path, ctx, layers
        ));
    }

    std::fs::write(&path, ini).map_err(|e| format!("写入 preset 失败: {e}"))?;
    Ok(path)
}

/// 估算能放进显存的层数。
///
/// `size_mb` 是量化后的权重体积，`layers` 缺省按 32 层算。
/// 预留：桌面/其他程序约 600MB、KV cache 与计算缓冲约体积的 4% + 200MB。
fn estimate_layers(size_mb: u64, layers: Option<u32>, free_mb: u64) -> u64 {
    let layers = layers.unwrap_or(32).max(1) as f64;
    let weights = size_mb as f64 * 1.06;
    let overhead = size_mb as f64 * 0.04 + 800.0;
    let budget = free_mb as f64 - overhead;

    if budget >= weights {
        return 999;
    }
    if budget <= 0.0 {
        return 0;
    }

    ((layers * (budget / weights)).floor() as u64).min(layers as u64)
}

/// 从 GGUF 头部读取 `*.block_count`，用于按比例分配 GPU 层数。
pub fn read_layer_count(path: &Path) -> Option<u32> {
    read_numeric_meta(path, "block_count").map(|v| v as u32)
}

/// 模型原生上下文长度（`*.context_length`），用于给用户提示"还能开多大"。
pub fn read_context_length(path: &Path) -> Option<u64> {
    read_numeric_meta(path, "context_length")
}

/// 模型自带的元数据能力，供界面显示与禁用控件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelCaps {
    pub supports_thinking: bool,
    pub trained_ctx: Option<u64>,
}

/// 读 GGUF 元数据要扫过十几万条词汇表，几十毫秒量级；
/// 设置抽屉每 2.5 秒刷新一次，所以按「路径 + 体积 + 修改时间」缓存结果。
pub fn model_caps(path: &Path) -> ModelCaps {
    fn cache() -> &'static Mutex<HashMap<String, ModelCaps>> {
        static CACHE: OnceLock<Mutex<HashMap<String, ModelCaps>>> = OnceLock::new();
        CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    let meta = std::fs::metadata(path).ok();
    let key = match &meta {
        Some(m) => {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("{}|{}|{}", path.display(), m.len(), mtime)
        }
        None => path.display().to_string(),
    };

    if let Some(hit) = cache().lock().ok().and_then(|c| c.get(&key).copied()) {
        return hit;
    }

    let caps = ModelCaps {
        supports_thinking: read_chat_template(path)
            .map(|t| supports_thinking(&t))
            .unwrap_or(false),
        trained_ctx: read_context_length(path),
    };

    if let Ok(mut guard) = cache().lock() {
        // 缓存键带修改时间，旧条目留着也不会命中，简单限一下大小
        if guard.len() > 128 {
            guard.clear();
        }
        guard.insert(key, caps);
    }

    caps
}

/// 读取模型自带的对话模板（`tokenizer.chat_template`）。
/// 用它判断模型到底支不支持思考，而不是靠模型名字猜。
pub fn read_chat_template(path: &Path) -> Option<String> {
    read_string_meta(path, "chat_template")
}

/// 模型是否支持思考开关。
///
/// 思路与 llama.cpp Web UI 一致：模板里必须真的引用
/// `enable_thinking` / `reasoning_effort` / `thinking_budget` 之一，
/// 或者带 `<think>` 这类思考标记，才算支持。翻译模型（如 Hy-MT2）两项都没有。
pub fn supports_thinking(template: &str) -> bool {
    const NAMES: [&str; 3] = ["enable_thinking", "reasoning_effort", "thinking_budget"];
    NAMES.iter().any(|n| template_references(template, n)) || template.contains("<think>")
}

/// 判断 `name` 是否真的出现在 jinja 的 `{{ }}` 或 `{% %}` 里，
/// 避免把模板注释或文档文字误判成"支持"。
fn template_references(template: &str, name: &str) -> bool {
    let mut from = 0;

    while let Some(at) = template[from..].find(name) {
        let at = from + at;

        let open = [template[..at].rfind("{{"), template[..at].rfind("{%")]
            .into_iter()
            .flatten()
            .max();

        if let Some(open) = open {
            let between = &template[open..at];
            // 两个标签之间不能已经闭合，否则说明变量在标签外面
            if !between.contains("}}") && !between.contains("%}") {
                return true;
            }
        }

        from = at + name.len();
    }

    false
}

fn read_string_meta(path: &Path, key_suffix: &str) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).ok()?;
    if &magic != b"GGUF" {
        return None;
    }

    let version = read_u32(&mut reader)?;
    if version < 2 {
        return None;
    }

    let _tensor_count = read_u64(&mut reader)?;
    let kv_count = read_u64(&mut reader)?;

    for _ in 0..kv_count {
        let key = read_string(&mut reader)?;
        let vtype = read_u32(&mut reader)?;

        if key.ends_with(key_suffix) {
            if vtype == 8 {
                return read_string(&mut reader);
            }
            skip_value(&mut reader, vtype, 0)?;
            continue;
        }

        skip_value(&mut reader, vtype, 0)?;
    }

    None
}

/// 在 GGUF 元数据里找一个以 `key_suffix` 结尾的数值项。
pub fn read_numeric_meta(path: &Path, key_suffix: &str) -> Option<u64> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).ok()?;
    if &magic != b"GGUF" {
        return None;
    }

    let version = read_u32(&mut reader)?;
    if version < 2 {
        return None;
    }

    let _tensor_count = read_u64(&mut reader)?;
    let kv_count = read_u64(&mut reader)?;

    for _ in 0..kv_count {
        let key = read_string(&mut reader)?;
        let vtype = read_u32(&mut reader)?;

        if key.ends_with(key_suffix) {
            match vtype {
                4 => return read_u32(&mut reader).map(|v| v as u64),
                10 => return read_u64(&mut reader),
                _ => {
                    skip_value(&mut reader, vtype, 0)?;
                    continue;
                }
            }
        }

        skip_value(&mut reader, vtype, 0)?;
    }

    None
}

fn read_u8<R: Read>(r: &mut R) -> Option<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b).ok()?;
    Some(b[0])
}

fn read_u32<R: Read>(r: &mut R) -> Option<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).ok()?;
    Some(u32::from_le_bytes(b))
}

fn read_u64<R: Read>(r: &mut R) -> Option<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b).ok()?;
    Some(u64::from_le_bytes(b))
}

fn read_string<R: Read>(r: &mut R) -> Option<String> {
    let len = read_u64(r)? as usize;
    // 对话模板可能有几 KB（Qwen3.5 约 7.8KB），上限留到 1MB
    if len > 1_048_576 {
        return None;
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}

/// 跳过任意类型的元数据值。
fn skip_value<R: Read>(r: &mut R, vtype: u32, depth: u8) -> Option<()> {
    if depth > 8 {
        return None;
    }
    match vtype {
        0 | 1 | 7 => {
            read_u8(r)?;
        }
        2 | 3 => {
            let mut b = [0u8; 2];
            r.read_exact(&mut b).ok()?;
        }
        4 | 5 | 6 => {
            let mut b = [0u8; 4];
            r.read_exact(&mut b).ok()?;
        }
        8 => {
            // 分词器词汇表动辄十几万条，这里不分配字符串，直接跳过字节
            skip_string(r)?;
        }
        9 => {
            let elem_type = read_u32(r)?;
            let count = read_u64(r)?;
            // 词汇表数组可以到十几万项，上限只做防爆保护
            if count > 10_000_000 {
                return None;
            }
            for _ in 0..count {
                skip_value(r, elem_type, depth + 1)?;
            }
        }
        10 | 11 | 12 => {
            let mut b = [0u8; 8];
            r.read_exact(&mut b).ok()?;
        }
        _ => return None,
    }
    Some(())
}

fn skip_string<R: Read>(r: &mut R) -> Option<()> {
    let len = read_u64(r)? as usize;
    skip_bytes(r, len)
}

/// 按块丢弃指定字节数，避免为分词器词汇表分配内存。
fn skip_bytes<R: Read>(r: &mut R, mut len: usize) -> Option<()> {
    let mut buf = [0u8; 8192];
    while len > 0 {
        let take = len.min(buf.len());
        r.read_exact(&mut buf[..take]).ok()?;
        len -= take;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_model_fits_fully_on_gpu() {
        // 1.1GB 模型，8GB 空闲：应全量上 GPU
        assert_eq!(estimate_layers(1100, Some(28), 7188), 999);
    }

    #[test]
    fn large_model_gets_partial_layers() {
        // 5.5GB 模型只有 3GB 空闲：应只放一部分层
        let layers = estimate_layers(5560, Some(40), 3000);
        assert!(layers > 0 && layers < 40, "got {layers}");
    }

    #[test]
    fn no_free_vram_falls_back_to_cpu() {
        assert_eq!(estimate_layers(5560, Some(40), 200), 0);
    }

    /// 用真实模型校验 GGUF 头部解析；未提供样例文件时自动跳过。
    #[test]
    fn parses_real_gguf_when_provided() {
        let Ok(sample) = std::env::var("LT_GGUF_SAMPLE") else {
            return;
        };
        let layers = read_layer_count(Path::new(&sample));
        assert!(
            layers.map(|n| n > 0).unwrap_or(false),
            "未能从 {sample} 解析出 block_count"
        );

        // 原生上下文同样来自 GGUF 元数据，用于给上下文档位设上限
        let ctx = read_context_length(Path::new(&sample));
        assert!(
            ctx.map(|n| n >= 512).unwrap_or(false),
            "未能从 {sample} 解析出 context_length"
        );

        // 对话模板用于判断是否支持思考，顺带打印出来供人核对
        let template = read_chat_template(Path::new(&sample));
        assert!(template.is_some(), "未能从 {sample} 解析出 chat_template");
        println!(
            "{sample} 支持思考: {}",
            supports_thinking(&template.unwrap_or_default())
        );
    }

    #[test]
    fn template_references_requires_jinja_tag() {
        assert!(template_references(
            "{%- if enable_thinking is defined %}",
            "enable_thinking"
        ));
        assert!(template_references("{{ enable_thinking }}", "enable_thinking"));
        // 注释里提到不算支持
        assert!(!template_references(
            "{# note: enable_thinking is supported #}",
            "enable_thinking"
        ));
        // 标签已闭合，变量在外面出现不算
        assert!(!template_references("{%- if x %} enable_thinking", "enable_thinking"));
    }

    #[test]
    fn supports_thinking_detects_think_markers() {
        assert!(supports_thinking("<|im_start|>assistant<think>"));
        assert!(supports_thinking(
            "{%- if reasoning_effort is defined %}high{%- endif %}"
        ));
        assert!(!supports_thinking("You are a professional translator."));
    }
}
