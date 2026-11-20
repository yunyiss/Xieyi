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
    /// GGUF 里的 KV 结构参数；读不到时为 。
    ///
    /// 前端用它**精确**算 KV cache 占多少显存，而不是按体积乘系数估。
    /// 读不到不是错误（老模型可能缺字段），调用方应退回估算。
    pub kv_shape: Option<KvShape>,
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
    out.sort_by_key(|a| a.id.to_lowercase());
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
            kv_shape: read_kv_shape(&path),
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
pub fn write_preset(
    app: &AppHandle,
    models: &[ModelFile],
    gpu_layers: u64,
) -> Result<PathBuf, String> {
    let root = paths::runtime_root(app)?;
    paths::ensure(&root)?;
    let path = root.join("presets.ini");

    let free_vram = if gpu_layers == AUTO_LAYERS {
        // 传引擎路径 → 走 `--list-devices`，AMD / Intel / Metal 都能拿到。
        // 拿不到再退回 nvidia-smi（见 hardware.rs 的兜底顺序）。
        let server = crate::runtime::read_info(app)
            .map(|i| std::path::PathBuf::from(i.server_path))
            .filter(|p| p.is_file());
        hardware::free_vram_mb(server.as_deref())
    } else {
        None
    };

    let mut ini = String::new();
    // b11714 起支持在 preset 里写 `log-verbosity`；只有打开它，引擎才会
    // 打印 `load_tensors: offloaded N/M layers` 和 `KV buffer size`，
    // 前端才能显示“实际分配”。老引擎不加，免得起不来。
    let verbose = crate::runtime::read_info(app)
        .and_then(|i| {
            i.version
                .strip_prefix('b')
                .and_then(|v| v.parse::<u32>().ok())
        })
        .is_some_and(|v| v >= 11714);
    let verbosity = if verbose { "log-verbosity = 4\n" } else { "" };
    // 先一次性取回所有模型参数：params::ctx_of 会每次重读 config.json，
    // 放在循环里就是「一个模型读一次盘」。
    let all_params = params::all(app);
    for m in models {
        let layers = if gpu_layers == AUTO_LAYERS {
            let layer_count = read_layer_count(Path::new(&m.path));
            free_vram
                .map(|free| estimate_layers(m.size_mb, layer_count, free))
                .unwrap_or(999)
        } else {
            gpu_layers
        };

        let ctx = params::from_map(&all_params, &m.id).ctx;

        ini.push_str(&format!(
            "[{}]\nmodel = {}\njinja = 1\nctx-size = {}\nn-gpu-layers = {}\ncache-type-k = q8_0\ncache-type-v = q8_0\n{}\n",
            m.id, m.path, ctx, layers, verbosity
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

/// GGUF 里与 KV cache 体积相关的结构参数。
///
/// 拿到这些就能**精确算出** KV cache 占多少显存，不必按"体积乘系数"估。
///
/// ## 为什么值得读
///
/// 早先前端用 `权重 × 1.25` 估算，问题有两个：
///
/// 1. **不随上下文长度变化**——用户把 ctx 从 4096 调到 32768，占用涨 8 倍，
///    估算却纹丝不动，于是把"放不下"算成"放得下"
/// 2. 系数是拍的，不同模型结构（尤其 GQA 的 kv_heads 差异）误差很大
///
/// 实测对照（Hy-MT2-1.8B，本机 RTX 4060，项目 preset 用 q8_0 KV）：
///
/// | ctx | 本公式 | 引擎 `KV buffer size` |
/// |---|---|---|
/// | 4096 | 128 MiB | 272 MiB（含 q8_0 块对齐开销）|
/// | 8192 | 256 MiB | 512 MiB |
///
/// 注意上表口径差异：**未显式传 `-ctk/-ctv` 时默认 f16，每元素 2 字节**，
/// 而我们的 preset 写的是 `cache-type-k/v = q8_0`（1 字节）。所以：
///
/// - 引擎实测（未传 ctk） = 本公式 × 2
/// - 引擎实测（q8_0）     = 本公式 + 块对齐开销
///
/// 计算时按项目**实际会用的** q8_0（1 字节）走，见 [`kv_cache_bytes`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KvShape {
    /// `*.block_count`，层数
    pub layers: u32,
    /// `*.attention.head_count_kv`，KV 头数
    ///
    /// 混合注意力模型里不同层可能不同，这里取一个代表值（最大头数）；
    /// 真正参与计算的是 [`Self::kv_head_layers`]。
    pub kv_heads: u32,
    /// `*.attention.key_length`；缺省时用 `embedding_length / head_count`
    pub key_len: u32,
    /// `*.attention.value_length`；缺省时同 `key_len`
    pub value_len: u32,
    /// **所有“全上下文”层的 KV 头数之和**。
    ///
    /// 普通模型 = `layers × kv_heads`；混合注意力模型只统计真正有
    /// 全上下文 KV 的层，否则会把 KV 高估好几倍。
    pub kv_head_layers: u64,
    /// 滑动窗口层的 KV 头数之和。Gemma 这类“局部 + 全局”混合模型才有；
    /// 这些层的 KV 只按窗口大小算，不按整个上下文算。
    pub local_kv_head_layers: u64,
    /// `*.attention.sliding_window`，局部注意力窗口；没有则为 `None`。
    pub sliding_window: Option<u32>,
}

impl KvShape {
    /// 按项目实际使用的 KV 缓存类型（q8_0，每元素 1 字节）算 KV 占用。
    ///
    /// 公式：`Σ(kv_heads × 有效 token 数) × (key_len + value_len) × 1`
    ///
    /// - 全上下文层：有效 token = ctx
    /// - 滑动窗口层：有效 token = min(ctx, window)
    ///
    /// `kv_unified = true`（llama.cpp 默认）时 KV 池是所有 slot 共享的，
    /// **不乘并行槽数**——实测 `-np 1` 与 `-np 4` 的 `KV buffer size` 相同。
    ///
    /// 前端只需要 shape 里的这些数，自己按 ctx 算——因为 ctx 是用户随时
    /// 可调的，后端算死了就过期了。
    #[cfg(test)]
    pub fn kv_cache_bytes(&self, ctx: u64) -> u64 {
        let ctx = ctx.max(1);
        let window = self.sliding_window.unwrap_or(0) as u64;
        let local_tokens = if window > 0 { ctx.min(window) } else { ctx };
        let weighted = self
            .kv_head_layers
            .saturating_mul(ctx)
            .saturating_add(self.local_kv_head_layers.saturating_mul(local_tokens));
        weighted.saturating_mul((self.key_len as u64).saturating_add(self.value_len as u64))
    }

    /// 从 GGUF 原始字段组装。
    ///
    /// `kv_heads_raw` 可能是单个数字（`"4"`），也可能是每层一个数字的
    /// 数组（`"4,4,0,4,..."`，0 表示该层没有 KV）。数组长度必须等于
    /// 层数，否则宁可返回 `None` 让调用方退回估算——错位比没有更危险。
    fn from_parts(
        layers: u32,
        kv_heads_raw: &str,
        key_len: u32,
        value_len: u32,
        full_attention_interval: Option<u32>,
        sliding_window: Option<u32>,
    ) -> Option<Self> {
        if layers == 0 || key_len == 0 || value_len == 0 {
            return None;
        }

        let raw = kv_heads_raw.trim();
        if raw.is_empty() {
            return None;
        }

        // 数组：每层一个 KV 头数，0 表示该层没有 KV（线性注意力等）。
        if raw.contains(',') {
            let list: Vec<u32> = raw
                .split(',')
                .map(|s| s.trim().parse::<u32>().ok())
                .collect::<Option<_>>()?;
            if list.len() != layers as usize {
                return None;
            }
            let kv_head_layers: u64 = list.iter().map(|v| *v as u64).sum();
            if kv_head_layers == 0 {
                return None;
            }
            let kv_heads = list.iter().copied().max().unwrap_or(0);
            return Some(Self {
                layers,
                kv_heads,
                key_len,
                value_len,
                kv_head_layers,
                local_kv_head_layers: 0,
                sliding_window: None,
            });
        }

        // 单个数字：普通模型，或“每隔 N 层一次全注意力”的混合模型。
        let kv_heads: u32 = raw.parse().ok()?;
        if kv_heads == 0 {
            return None;
        }

        let full_layers = match full_attention_interval {
            Some(interval) if interval > 1 => layers.div_ceil(interval),
            _ => layers,
        };
        let local_layers = layers.saturating_sub(full_layers);

        // 有滑动窗口信息时，局部层按窗口大小算；没有就保守地按全上下文算，
        // 避免把“局部层也有 KV”的模型估少。
        let (full, local) = if local_layers > 0 && sliding_window.is_some_and(|w| w > 0) {
            (
                full_layers as u64 * kv_heads as u64,
                local_layers as u64 * kv_heads as u64,
            )
        } else {
            (layers as u64 * kv_heads as u64, 0)
        };

        Some(Self {
            layers,
            kv_heads,
            key_len,
            value_len,
            kv_head_layers: full,
            local_kv_head_layers: local,
            sliding_window: if local > 0 { sliding_window } else { None },
        })
    }
}

/// 从 GGUF 头读 KV 结构参数。
///
/// 复用 [`read_meta_many`]——它一次扫过整个 KV 段就能拿到多个字段，
/// 比逐字段调用省掉重复扫盘。
///
/// 任何一项缺失都返回 `None`：宁可让调用方退回估算，也不要用默认值
/// 编出一个看似精确、实则错误的结果。
pub fn read_kv_shape(path: &Path) -> Option<KvShape> {
    const KEYS: &[&str] = &[
        "block_count",
        "attention.head_count_kv",
        "attention.key_length",
        "attention.value_length",
        "full_attention_interval",
        "attention.sliding_window",
    ];
    let m = read_meta_many(path, &[], KEYS);
    let get = |k: &str| m.get(k).and_then(|v| v.parse::<u32>().ok());

    let layers = get("block_count")?;
    // head_count_kv 可能是标量，也可能是逗号分隔的数组；原样交给 from_parts。
    let kv_heads_raw = m.get("attention.head_count_kv")?;
    // key/value 长度未必存在（老模型可能只给 embedding_length + head_count），
    // 缺了就放弃——本函数存在的意义就是"精确"，估出来的精确是假精确。
    let key_len = get("attention.key_length")?;
    let value_len = get("attention.value_length")?;

    KvShape::from_parts(
        layers,
        kv_heads_raw,
        key_len,
        value_len,
        get("full_attention_interval"),
        get("attention.sliding_window"),
    )
}

/// 从 GGUF 头部读取 `*.block_count`，用于按比例分配 GPU 层数。
///
/// ## 为什么要缓存
///
/// 这个函数只在 `write_preset` 里被调用，而 `write_preset` 每次启动引擎
/// 都会跑一遍（改模型目录、换引擎、重启服务都会触发）。它按模型逐个
/// 打开文件、**线性遍历整个 KV 段**直到找到 `block_count`——模型越多、
/// 元数据越多，耗时越长。
///
/// 而同一个文件头在 `model_caps` 里已经被扫过一次并缓存了。这里再加
/// 一层同样策略的缓存（键 = 路径 + 体积 + 修改时间），重启引擎时就不必
/// 重新扫盘。缓存失效条件是文件被替换，与 `model_caps` 完全一致。
pub fn read_layer_count(path: &Path) -> Option<u32> {
    // 缓存键只按**路径**，值的有效期用 (体积, mtime) 校验。
    //
    // 早先直接把体积和 mtime 拼进键里，结果文件一旦不可读（改名、
    // 临时锁、被移动）metadata 拿不到，键就退化成一个新的形状，
    // 与之前那条缓存对不上 —— 于是"文件不可读"反而触发了一次扫盘。
    // 键按路径走、值里带指纹，才既能命中又能及时发现文件被替换。
    let key = path.display().to_string();

    let now = layer_cache_fingerprint(path);
    if let Some(hit) = layer_cache_lookup(&key, now) {
        return hit;
    }

    let value = read_numeric_meta(path, "block_count").map(|v| v as u32);
    layer_cache_store(key, now, value);
    value
}

/// layer_count 缓存的指纹：(体积, mtime 秒)。
///
/// 拿不到元数据时用 `(0, 0)` 哨兵——不与任何真实文件相同，
/// 因此文件不可读时缓存必定失效，会老实去解析一次。
fn layer_cache_fingerprint(path: &Path) -> (u64, u64) {
    match std::fs::metadata(path) {
        Ok(m) => (
            m.len(),
            m.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
        ),
        Err(_) => (0, 0),
    }
}

/// 文件指纹：(体积, mtime 秒)。
type LayerFingerprint = (u64, u64);

/// layer_count 缓存：路径 → (指纹, 层数)。
///
/// 值里带指纹而不是把指纹拼进键，理由见 `read_layer_count` 的注释。
type LayerCache = HashMap<String, (LayerFingerprint, Option<u32>)>;

fn layer_cache() -> &'static Mutex<LayerCache> {
    static CACHE: OnceLock<Mutex<LayerCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 命中则返回值。指纹不一致视为未命中（同时把陈旧条目留在原地，
/// 由下一次 `store` 覆盖——反正键一样）。
fn layer_cache_lookup(key: &str, fingerprint: LayerFingerprint) -> Option<Option<u32>> {
    let guard = layer_cache().lock().ok()?;
    let (fp, value) = guard.get(key)?;
    if *fp == fingerprint {
        Some(*value)
    } else {
        None
    }
}

fn layer_cache_store(key: String, fingerprint: LayerFingerprint, value: Option<u32>) {
    if let Ok(mut guard) = layer_cache().lock() {
        if guard.len() > 128 {
            guard.clear();
        }
        guard.insert(key, (fingerprint, value));
    }
}

/// 模型原生上下文长度（`*.context_length`），用于给用户提示"还能开多大"。
///
/// 生产路径已改走 [`read_meta_many`]（一次扫描取多项），
/// 这里保留给测试与后续按需单读使用。
#[cfg(test)]
pub fn read_context_length(path: &Path) -> Option<u64> {
    read_numeric_meta(path, "context_length")
}

/// 一个模型该用哪种**调用机制**（而不是"它是什么型号"）。
///
/// 不同翻译模型的接口差异很大：有的直接用英文 system prompt，有的要求中文
/// 文本提示，有的把语言码放在 `chat_template_kwargs`，还有的把语言对写进
/// system 消息。用错机制不一定报错——更糟，它会**静默给出错误结果**
/// （例如 translategemma 不传语言码就把一切翻成 en-GB）。
///
/// 判定依据全部来自 GGUF 元数据里的 `chat_template` 特征词与模型名，
/// 不维护型号名单：新模型一个月出好几个，名单必然滞后。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PromptStyle {
    /// 英文 system prompt。现有实现，通用 chat 模型走这条。
    #[default]
    EnglishSystem,
    /// 中文文本提示、不发 system。Index-Translate 官方模型卡给的格式。
    ChineseText,
    /// 语言码走 `chat_template_kwargs`（translategemma 原版）。
    TemplateKwargs,
    /// 内容里拼 `<<<source>>>zh<<<target>>>en<<<text>>>`（translategemma 的 vLLM 优化版）。
    Delimiter,
    /// system 消息写 `zh-en` 这类语言对代码（NVIDIA Riva-Translate）。
    LanguagePair,
    /// **官方训练用的极简中文模板**，single user message。
    ///
    /// 腾讯 Hunyuan-MT 技术报告 Table 1 给出的原始格式：
    ///   涉及中文的方向：`把下面的文本翻译成<target_language>，不要额外解释。`
    ///   其他方向：     `Translate the following segment into <target_language>, without additional explanation.`
    ///
    /// 为什么单独成一条：Hy-MT2 是**按中文模板训练的**，喂英文 system prompt
    /// 会让 1.8B 这类小模型失常——实测 `hello` 原样吐回、`Apple` 不翻；
    /// 换成官方模板后全部正常，繁体也从"永远给简体"变成可用。
    /// 长篇英文提示对它不是"更清楚"，而是"没见过的格式"。
    ChineseOfficial,
}

impl PromptStyle {
    /// 线格式名称，前端与服务日志共用。
    pub fn as_str(self) -> &'static str {
        match self {
            PromptStyle::EnglishSystem => "english-system",
            PromptStyle::ChineseText => "chinese-text",
            PromptStyle::TemplateKwargs => "template-kwargs",
            PromptStyle::Delimiter => "delimiter",
            PromptStyle::LanguagePair => "language-pair",
            PromptStyle::ChineseOfficial => "chinese-official",
        }
    }
}

/// 模型的调用画像：怎么发请求、能不能拿来对话、用不了时为什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelProfile {
    pub style: PromptStyle,
    /// 能否进对话页。翻译专用模型为 false——实测它们会把问题本身翻成英文，
    /// 或者编造身份，放在对话页只会让用户以为工具坏了。
    pub chat_capable: bool,
    /// 面向用户的家族名（"Hy-MT2"、"TranslateGemma"…），用于界面徽章。
    pub family: String,
    /// 完全用不了时的原因；为空表示可用。
    pub unsupported: Option<String>,
}

impl Default for ModelProfile {
    fn default() -> Self {
        Self {
            style: PromptStyle::EnglishSystem,
            chat_capable: true,
            family: "通用".into(),
            unsupported: None,
        }
    }
}

/// 模型自带的元数据能力，供界面显示与禁用控件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelCaps {
    pub supports_thinking: bool,
    pub trained_ctx: Option<u64>,
    pub profile: ModelProfile,
}

/// 从模型名与对话模板推断调用画像。
///
/// 顺序有讲究：先判"完全用不了"，再判"需要特殊参数"，最后才回落到通用。
/// 最后一步的兜底是 `EnglishSystem` + 可对话——**绝不能误伤用户自选的
/// Qwen / Llama 等通用模型**，那是这个工具最常见的用法。
pub fn profile_of(name: &str, template: Option<&str>) -> ModelProfile {
    let lower = name.to_lowercase();
    let tpl = template.unwrap_or("");

    // ① Seed-X：官方模型卡明确写了没有 chat 模板、不要把对话格式喂给它，
    //    并且不建议使用非官方量化版。实测确实会输出幻觉与乱码。
    //    注意 GGUF 里的 general.name 常写成 "Seed X PPO 7B"（空格分隔），
    //    所以要先把空格/下划线归一化再匹配。
    let squashed = lower.replace([' ', '_'], "");
    if squashed.contains("seed-x") || squashed.contains("seedx") {
        return ModelProfile {
            style: PromptStyle::EnglishSystem,
            chat_capable: false,
            family: "Seed-X".into(),
            unsupported: Some(
                "该模型没有官方对话模板，且官方不建议使用非官方量化版；\
                 实测会输出与原文无关的内容。请换用 Hy-MT2、Index-Translate 或通用 chat 模型。"
                    .into(),
            ),
        };
    }

    // ② 已知家族：能翻译，也**能拿来对话**——但需要各自的最佳调用方式。
    //
    // 这里有个踩过的坑：最初按"翻译模型一律不能对话"处理，结果用户模型目录里
    // 只有翻译模型时，对话页直接空了。实测推翻了这个判断：
    //   Hy-MT2-7B        改写任务 ✅「该功能性能优异，建议您予以试用。」
    //                    闲聊     ✅「我是混元，是由腾讯开发的大模型。」
    //   Index-Translate  改写任务 ✅「此功能颇为实用，不妨一试。」（需避开思考吃光额度）
    // 它们明确不是"不能对话"，而是"对话时要用对提示词"。
    // 真正不能对话的只有 Seed-X（官方无 chat 模板）和单轮翻译器（translategemma，
    // 会把用户的问题本身翻译掉）。
    let known = |family: &str, style: PromptStyle| ModelProfile {
        style,
        chat_capable: true,
        family: family.into(),
        unsupported: None,
    };

    // ---- 真正不能对话的：单轮翻译器 ----
    // 它们把**用户的问题本身**当成待翻译文本，问"你是谁"会回一句英文，
    // 因为模板里写死了"把这段翻译成目标语言"，没有留出"回答指令"的位置。
    let translation_only = |family: &str, style: PromptStyle| ModelProfile {
        style,
        chat_capable: false,
        family: family.into(),
        unsupported: None,
    };

    // translategemma：语言码必须走 chat_template_kwargs，否则默认 en-GB，
    // 把任何输入都翻成英式英语。它的 vLLM 优化版改用文本分隔符。
    if tpl.contains("source_lang_code") || tpl.contains("target_lang_code") {
        return translation_only("TranslateGemma", PromptStyle::TemplateKwargs);
    }
    if tpl.contains("<<<source>>>") || tpl.contains("<<<text>>>") {
        return translation_only("TranslateGemma", PromptStyle::Delimiter);
    }
    // NVIDIA Riva-Translate：模板内嵌 language_pairs，system 写 "zh-en"。
    if tpl.contains("language_pairs") {
        return translation_only("Riva-Translate", PromptStyle::LanguagePair);
    }
    if squashed.contains("translategemma") || squashed.contains("translate-gemma") {
        // 名字像但模板没特征：仍按 translategemma 处理，宁可多传参数
        return translation_only("TranslateGemma", PromptStyle::TemplateKwargs);
    }

    // Index-Translate（B 站）：官方用中文文本提示，并支持术语/格式硬约束。
    // 实测能对话（「此功能颇为实用，不妨一试。」），只是直接问答时容易被
    // 思维链吃光输出额度，需要用它的约束格式引导。
    if squashed.contains("index-translate") || squashed.contains("indextranslate") {
        return known("Index-Translate", PromptStyle::ChineseText);
    }

    // ③ Hy-MT2：必须用它训练时的中文模板。
    //
    // 这条曾经写成 EnglishSystem（"指令服从度好，英文 system prompt 就能用"），
    // 后来实测推翻了：7B 确实能吃英文提示，但 **1.8B 会失常**——
    // `hello` 原样返回、`Apple` 不翻、要繁体却永远给简体。
    // 换成腾讯技术报告 Table 1 的官方模板后全部恢复正常。
    //
    // 教训：翻译专用小模型是按特定模板训练的，英文改写是"没见过的格式"，
    // 不是"更清楚的指令"。
    // 对话页改写与闲聊仍正常，属于"能对话"。
    if squashed.contains("hy-mt") || squashed.contains("hymt") || squashed.contains("hunyuan-mt") {
        return known("Hy-MT2", PromptStyle::ChineseOfficial);
    }

    // ④ 兜底：通用对话模型。宁可当作"能对话"，也不要误判。
    ModelProfile {
        style: PromptStyle::EnglishSystem,
        chat_capable: true,
        family: "通用".into(),
        unsupported: None,
    }
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

    if let Some(hit) = cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }

    // 名字用于家族判定。**优先用文件名**：GGUF 里的 `general.name` 完全不可靠，
    // 实测 Hy-MT2-1.8B 写着 "Global_Step_300"（训练检查点残留）、
    // Index-Translate-9B 只写 "9b"。文件名才是用户真正下载的那个模型。
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();

    // 一次扫描取全部需要的元数据（分四次调用要重扫四遍文件头）
    let meta = read_meta_many(
        path,
        &["general.name", "general.basename", "chat_template"],
        &["context_length"],
    );
    let meta_name = meta
        .get("general.name")
        .or_else(|| meta.get("general.basename"))
        .cloned()
        .unwrap_or_default();
    // 两个都试：文件名抓主流情况，元数据名兜底（有些模型文件名被改过）
    let name = format!("{stem} {meta_name}");

    let template = meta.get("chat_template").cloned();
    let caps = ModelCaps {
        supports_thinking: template.as_deref().map(supports_thinking).unwrap_or(false),
        trained_ctx: meta.get("context_length").and_then(|v| v.parse().ok()),
        profile: profile_of(&name, template.as_deref()),
    };

    if let Ok(mut guard) = cache().lock() {
        // 缓存键带修改时间，旧条目留着也不会命中，简单限一下大小
        if guard.len() > 128 {
            guard.clear();
        }
        guard.insert(key, caps.clone());
    }

    caps
}

/// 读取模型自带的对话模板（`tokenizer.chat_template`）。
/// 用它判断模型到底支不支持思考，而不是靠模型名字猜。
///
/// 生产路径已改走 [`read_meta_many`]，这里保留给测试使用。
#[cfg(test)]
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

/// 一次扫描读出多个字符串/数字元数据。
///
/// `read_string_meta` 每调用一次都要重新打开文件、从头部扫过整个 KV 区；
/// 而判定模型画像要看 name / basename / chat_template / context_length 四项，
/// 分开调用就是四趟。这里合并成一趟，模型多时差别很明显。
///
/// 返回的 map 里键是**传入的 suffix 本身**（不是 GGUF 里的完整键名），
/// 值统一用字符串表示，数字键由调用方自己解析。
fn read_meta_many(
    path: &Path,
    string_suffixes: &[&str],
    numeric_suffixes: &[&str],
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(file) = std::fs::File::open(path) else {
        return out;
    };
    let mut reader = BufReader::new(file);

    let mut magic = [0u8; 4];
    if reader.read_exact(&mut magic).is_err() || &magic != b"GGUF" {
        return out;
    }
    let Some(version) = read_u32(&mut reader) else {
        return out;
    };
    if version < 2 {
        return out;
    }
    let Some(_tensor_count) = read_u64(&mut reader) else {
        return out;
    };
    let Some(kv_count) = read_u64(&mut reader) else {
        return out;
    };

    let want_string = |k: &str| string_suffixes.iter().find(|s| k.ends_with(**s)).copied();
    let want_number = |k: &str| numeric_suffixes.iter().find(|s| k.ends_with(**s)).copied();

    for _ in 0..kv_count {
        let Some(key) = read_string(&mut reader) else {
            break;
        };
        let Some(vtype) = read_u32(&mut reader) else {
            break;
        };

        // 字符串键：直接把值读出来
        if let Some(suffix) = want_string(&key)
            && vtype == 8
        {
            if let Some(value) = read_string(&mut reader) {
                out.entry(suffix.to_string()).or_insert(value);
            }
            continue;
        }

        // 数字键：读成字符串存（调用方自己 parse）
        if let Some(suffix) = want_number(&key) {
            match vtype {
                4 => {
                    if let Some(v) = read_u32(&mut reader) {
                        out.entry(suffix.to_string()).or_insert(v.to_string());
                        continue;
                    }
                    return out;
                }
                10 => {
                    if let Some(v) = read_u64(&mut reader) {
                        out.entry(suffix.to_string()).or_insert(v.to_string());
                        continue;
                    }
                    return out;
                }
                // 数组：每层一个数字（如 head_count_kv）。转成逗号分隔字符串，
                // 让上层能区分“标量”和“逐层数组”。
                9 => {
                    let Some(elem_type) = read_u32(&mut reader) else {
                        return out;
                    };
                    let Some(count) = read_u64(&mut reader) else {
                        return out;
                    };
                    if count > 100_000 {
                        return out;
                    }
                    if elem_type == 4 || elem_type == 10 {
                        let mut vals = Vec::with_capacity(count as usize);
                        for _ in 0..count {
                            let v = if elem_type == 4 {
                                match read_u32(&mut reader) {
                                    Some(v) => v as u64,
                                    None => return out,
                                }
                            } else {
                                match read_u64(&mut reader) {
                                    Some(v) => v,
                                    None => return out,
                                }
                            };
                            vals.push(v.to_string());
                        }
                        out.entry(suffix.to_string()).or_insert(vals.join(","));
                        continue;
                    }
                    // 元素类型不是整数：跳过整个数组，继续往后扫。
                    for _ in 0..count {
                        if skip_value(&mut reader, elem_type, 0).is_none() {
                            return out;
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }

        if skip_value(&mut reader, vtype, 0).is_none() {
            break;
        }
    }

    out
}

/// 读单个字符串元数据（重开文件、从头扫一遍）。
/// 需要多项时请用 [`read_meta_many`]，别连着调这个。
#[cfg(test)]
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
        4..=6 => {
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
        10..=12 => {
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

    /// 缓存必须真的命中：第二次调用**不再扫文件头**。
    ///
    /// `write_preset` 每次启动引擎都会对每个模型调一次 `read_layer_count`，
    /// 而它要线性遍历整个 KV 段。没有缓存时，"启动引擎"的开销随模型
    /// 数量线性增长。
    ///
    /// 这里直接用缓存探针判断，不靠耗时——耗时在快盘上噪声太大，
    /// 变异验证时根本抓不住"缓存被绕过"（试过，确实抓不住）。
    #[test]
    fn layer_count_is_cached_after_first_read() {
        let Ok(sample) = std::env::var("LT_GGUF_SAMPLE") else {
            return;
        };
        let tmp = std::env::temp_dir().join("xieyi-cache-test.gguf");
        if std::fs::copy(&sample, &tmp).is_err() {
            return; // 临时目录不可写就跳过，不把环境问题当失败
        }

        let value = read_layer_count(&tmp);
        assert!(value.is_some(), "首次读取应当成功");

        // 缓存里必须留下这条记录，且指纹与当前文件一致
        let key = tmp.display().to_string();
        let fp = layer_cache_fingerprint(&tmp);
        let cached = layer_cache_lookup(&key, fp);
        assert_eq!(
            cached,
            Some(value),
            "首次读取后必须写入缓存，否则每次启动引擎都会重扫全部模型"
        );

        // 指纹变化（文件被替换）时不能命中
        assert_eq!(
            layer_cache_lookup(&key, (fp.0 + 1, fp.1)),
            None,
            "文件指纹变了还命中缓存 = 换掉 .gguf 后仍沿用旧层数"
        );

        let _ = std::fs::remove_file(&tmp);
    }

    /// 文件被替换后，缓存必须失效并重新解析。
    ///
    /// 这是上一条的对偶：缓存不能只看路径，否则用户换掉 .gguf 后
    /// 层数会一直沿用旧值，显存估算就错了。
    #[test]
    fn layer_count_cache_invalidates_when_file_changes() {
        let Ok(sample) = std::env::var("LT_GGUF_SAMPLE") else {
            return;
        };
        let tmp = std::env::temp_dir().join("xieyi-cache-invalidate.gguf");
        if std::fs::copy(&sample, &tmp).is_err() {
            return;
        }

        let first = read_layer_count(&tmp);
        assert!(first.is_some(), "首次读取应当成功");

        // 破坏文件内容（保留文件名），指纹随之改变
        if std::fs::write(&tmp, b"NOTAGGUF").is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }

        let after = read_layer_count(&tmp);
        assert!(
            after.is_none(),
            "文件内容已变，缓存必须失效并重新解析（旧值 {first:?} 不该被沿用）"
        );

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn template_references_requires_jinja_tag() {
        assert!(template_references(
            "{%- if enable_thinking is defined %}",
            "enable_thinking"
        ));
        assert!(template_references(
            "{{ enable_thinking }}",
            "enable_thinking"
        ));
        // 注释里提到不算支持
        assert!(!template_references(
            "{# note: enable_thinking is supported #}",
            "enable_thinking"
        ));
        // 标签已闭合，变量在外面出现不算
        assert!(!template_references(
            "{%- if x %} enable_thinking",
            "enable_thinking"
        ));
    }

    #[test]
    fn supports_thinking_detects_think_markers() {
        assert!(supports_thinking("<|im_start|>assistant<think>"));
        assert!(supports_thinking(
            "{%- if reasoning_effort is defined %}high{%- endif %}"
        ));
        assert!(!supports_thinking("You are a professional translator."));
    }

    // ---- 调用机制识别 ----

    /// **最重要的一条**：认不出来的必须当通用模型，绝不能误判成翻译专用。
    /// 用户放个 Qwen / Llama 进来是常态，误判会让他的对话页直接空掉。
    #[test]
    fn unknown_models_fall_back_to_generic_chat() {
        for (name, template) in [
            (
                "Qwen3.5-9B",
                Some("<|im_start|>{{ message.role }}\n{{ message.content }}<|im_end|>"),
            ),
            (
                "llama-3.1-8b",
                Some("{{ bos_token }}{% for m in messages %}..."),
            ),
            ("gemma-3-4b", None),
            ("", None),
        ] {
            let p = profile_of(name, template);
            assert_eq!(p.style, PromptStyle::EnglishSystem, "{name} 不该被特殊对待");
            assert!(p.chat_capable, "{name} 应当能在对话页使用");
            assert_eq!(p.family, "通用");
            assert!(p.unsupported.is_none());
        }
    }

    /// translategemma 必须靠 kwargs 传语言码，否则默认 en-GB 把一切翻成英语。
    #[test]
    fn translategemma_detected_by_template() {
        let tpl = "{%- set languages = {...} -%}{{ content['source_lang_code'] }}";
        let p = profile_of("translategemma-4b-it", Some(tpl));
        assert_eq!(p.style, PromptStyle::TemplateKwargs);
        assert_eq!(p.family, "TranslateGemma");
        // 它没有对话能力：喂任何问题都会被翻译成英语
        assert!(!p.chat_capable);
    }

    #[test]
    fn translategemma_delimiter_variant_detected() {
        let tpl = "content = '<<<source>>>' + src + '<<<target>>>' + tgt + '<<<text>>>' + text";
        let p = profile_of("vllm-translategemma-4b-it", Some(tpl));
        assert_eq!(p.style, PromptStyle::Delimiter);
    }

    /// 名字像但模板没特征词时，仍按 translategemma 处理——
    /// 宁可多传一个用不上的参数，也不要它把内容翻错语言。
    #[test]
    fn translategemma_detected_by_name_when_template_plain() {
        let p = profile_of("translategemma-12b-it", Some("{{ messages[0].content }}"));
        assert_eq!(p.style, PromptStyle::TemplateKwargs);
    }

    #[test]
    fn riva_translate_uses_language_pair_system() {
        let tpl = "{%- set language_pairs = {'en-zh': {...}} -%}";
        let p = profile_of("Riva-Translate-4B-Instruct-v2", Some(tpl));
        assert_eq!(p.style, PromptStyle::LanguagePair);
        assert_eq!(p.family, "Riva-Translate");
        assert!(!p.chat_capable);
    }

    #[test]
    fn index_translate_uses_chinese_prompt_and_can_chat() {
        let p = profile_of(
            "Index-Translate-9B",
            Some("<|im_start|>{{ message.content }}"),
        );
        assert_eq!(p.style, PromptStyle::ChineseText);
        assert_eq!(p.family, "Index-Translate");
        // 实测能对话（「此功能颇为实用，不妨一试。」），只是要用中文约束格式
        assert!(p.chat_capable);
    }

    /// Hy-MT2 改用**官方中文模板**，仍然能对话。
    ///
    /// 这条断言前后翻过两次，两次都是为了修实际 bug：
    ///   1. 曾判成"不能对话" → 用户模型目录只有翻译模型时对话页全空，改回 true
    ///   2. 曾用 EnglishSystem → 1.8B 短词原样返回、要繁体给简体，
    ///      改成官方 ChineseOfficial 后恢复正常（7B 靠体量能扛住，1.8B 扛不住）
    ///
    /// 对话能力不变：改写「该功能性能优异，建议您予以试用。」、闲聊「我是混元…」都正常。
    #[test]
    fn hy_mt2_uses_official_chinese_template_and_can_chat() {
        for name in ["Hy-MT2-1.8B", "Hy-MT2-7B", "Hunyuan-MT-7B"] {
            let p = profile_of(name, Some("<|startoftext|>{{ message.content }}"));
            assert_eq!(p.style, PromptStyle::ChineseOfficial, "{name}");
            assert_eq!(p.family, "Hy-MT2", "{name}");
            assert!(p.chat_capable, "{name} 实测能对话，不该被挡掉");
        }
    }

    /// Seed-X 官方没有对话模板、且不建议用非官方量化版，实测输出幻觉。
    /// 必须明确标成不可用，而不是让它跑出乱码。
    #[test]
    fn seed_x_is_marked_unsupported() {
        let p = profile_of("Seed X PPO 7B", Some("<|im_start|>{{ message.content }}"));
        assert_eq!(p.family, "Seed-X");
        assert!(!p.chat_capable);
        let reason = p.unsupported.as_deref().unwrap_or_default();
        assert!(reason.contains("对话模板"), "原因要说清楚: {reason}");
    }

    /// GGUF 的 `general.name` 不可靠，家族判定必须能吃到文件名。
    ///
    /// 实测：Hy-MT2-1.8B 的 general.name 是 `Global_Step_300`（训练检查点残留），
    /// Index-Translate-9B 只写 `9b`。只看元数据名会把它们全判成"通用"，
    /// 于是翻译专用模型混进对话页、Index-Translate 也用不上中文提示。
    #[test]
    fn family_detection_survives_useless_metadata_names() {
        // 文件名 + 无意义的元数据名拼在一起，仍要识别出家族
        let p = profile_of(
            "Hy-MT2-1.8B-Q8_0 Global_Step_300",
            Some("<|startoftext|>{{ x }}"),
        );
        assert_eq!(p.family, "Hy-MT2");

        let p = profile_of("Index-Translate-9B 9b", Some("<|im_start|>{{ x }}"));
        assert_eq!(p.family, "Index-Translate");
        assert_eq!(p.style, PromptStyle::ChineseText);
    }

    /// **防误判**：名字里带关键词、但实际是通用模型的，不能被判定成翻译模型。
    ///
    /// 这条是给"以后加新模型"准备的。用户目录里会不断出现新模型，
    /// 一旦某个通用模型的文件名恰好含 translate / index / seed / hy，
    /// 误判会让它在对话页凭空消失——比不认识更糟。
    #[test]
    fn generic_models_with_suspicious_names_stay_generic() {
        let cases = [
            ("Qwen3.5-9B-UD-Q4_K_XL Qwen3.5-9B", "<|im_start|>{{ x }}"),
            ("Meta-Llama-3.1-8B-Instruct Llama 3.1", "{{ bos_token }}"),
            ("gemma-4-E4B-it gemma-4", "<start_of_turn>user"),
            ("DeepSeek-R1-Distill-Qwen-7B deepseek", "{{ x }}"),
            // 文件名带关键词但都不是对应家族
            ("translation-helper-chat-v1 assistant", "{{ x }}"),
            ("index-embedding-bge-m3 bge", "{{ x }}"),
            ("hy3-preview hy3", "{{ x }}"),
            ("Seed-OSS-36B seed-oss", "{{ x }}"),
        ];

        for (name, tpl) in cases {
            let p = profile_of(name, Some(tpl));
            assert_eq!(p.family, "通用", "{name} 被误判成 {}", p.family);
            assert!(p.chat_capable, "{name} 被误判成不可对话");
            assert_eq!(
                p.style,
                PromptStyle::EnglishSystem,
                "{name} 用了错误的调用机制"
            );
            assert!(p.unsupported.is_none(), "{name} 不该被标成不可用");
        }
    }

    #[test]
    fn prompt_style_wire_names_are_stable() {
        // 线格式名一旦改了，前端与日志都得跟着改，所以钉住
        assert_eq!(PromptStyle::EnglishSystem.as_str(), "english-system");
        assert_eq!(PromptStyle::ChineseText.as_str(), "chinese-text");
        assert_eq!(PromptStyle::TemplateKwargs.as_str(), "template-kwargs");
        assert_eq!(PromptStyle::Delimiter.as_str(), "delimiter");
        assert_eq!(PromptStyle::LanguagePair.as_str(), "language-pair");
    }

    /// 真实模型的识别结果必须稳定：这是"适配层对不对"的总闸门。
    ///
    /// 用本机实测过的模型名 + 几个刻意构造的陷阱名，断言家族、调用机制、
    /// 能否对话三项都符合预期。
    ///
    /// **这条测试的价值在于它不依赖外部文件**：`profile_of` 只看文件名与
    /// chat_template 特征词，所以能在任何机器上跑，不需要真实 .gguf。
    /// 新增家族时往这里加一行即可（这也是"要不要为每个模型单独维护"
    /// 的答案：只在**新增一个家族**时改一处，不是每个模型都要写配置）。
    #[test]
    fn real_world_models_are_recognized_correctly() {
        // (文件名, 期望家族, 期望机制, 能否对话)
        let cases: &[(&str, &str, PromptStyle, bool)] = &[
            // 本机实测过的翻译模型
            (
                "Hy-MT2-1.8B-Q8_0.gguf",
                "Hy-MT2",
                PromptStyle::ChineseOfficial,
                true,
            ),
            (
                "Hy-MT2-7B-Q4_K_M.gguf",
                "Hy-MT2",
                PromptStyle::ChineseOfficial,
                true,
            ),
            (
                "Index-Translate-9B.Q4_K_M.gguf",
                "Index-Translate",
                PromptStyle::ChineseText,
                true,
            ),
            (
                "translategemma-12b-it-Q4_K_S.gguf",
                "TranslateGemma",
                PromptStyle::TemplateKwargs,
                false,
            ),
            (
                "translategemma-4b-it.Q8_0.gguf",
                "TranslateGemma",
                PromptStyle::TemplateKwargs,
                false,
            ),
            // 通用对话模型：一个都不能被误判
            (
                "Qwen3-8B-Q4_K_M.gguf",
                "通用",
                PromptStyle::EnglishSystem,
                true,
            ),
            (
                "gemma-3-4b-it.gguf",
                "通用",
                PromptStyle::EnglishSystem,
                true,
            ),
            (
                "Llama-3.1-8B-Instruct.gguf",
                "通用",
                PromptStyle::EnglishSystem,
                true,
            ),
            // 陷阱名：看着像某家族，实际不是
            (
                "Seoul-Translate-Bot.gguf",
                "通用",
                PromptStyle::EnglishSystem,
                true,
            ),
            (
                "hymn-generator-7b.gguf",
                "通用",
                PromptStyle::EnglishSystem,
                true,
            ),
        ];

        for (name, family, style, chat) in cases {
            let p = profile_of(name, None);
            assert_eq!(&p.family, family, "{name} 家族判错");
            assert_eq!(&p.style, style, "{name} 调用机制判错");
            assert_eq!(p.chat_capable, *chat, "{name} 能否对话判错");
            assert!(p.unsupported.is_none(), "{name} 不该被标成不可用");
        }
    }
}

#[cfg(test)]
mod kvshape_live {
    use super::*;

    /// 用本机真实模型验证 KV 参数读取与体积计算。
    ///
    /// 默认 `#[ignore]`：需要真实 GGUF 文件，CI 上没有。改这块代码后用：
    ///
    /// ```bash
    /// cargo test -- --ignored --nocapture kvshape_live
    /// ```
    ///
    /// ## 期望值从哪来
    ///
    /// 不是猜的。`llama-server -v` 会打印
    /// `llama_kv_cache: Vulkan0 KV buffer size = N MiB`，本机实测：
    ///
    /// | 模型 | ctx | 未传 ctk（f16） | `-ctk q8_0` |
    /// |---|---|---|---|
    /// | Hy-MT2-1.8B | 4096 | 256 MiB | — |
    /// | Hy-MT2-1.8B | 8192 | 512 MiB | 272 MiB |
    ///
    /// 本函数按项目 preset 实际用的 **q8_0（1 字节/元素）** 计算，
    /// 所以对应的是「272 MiB」那一列的量级（差异来自 q8_0 块对齐）。
    #[test]
    #[ignore]
    fn kvshape_live() {
        for p in [
            "D:/Model/translate/Hy-MT2-1.8B-Q8_0.gguf",
            "D:/Model/translate/Hy-MT2-7B-Q4_K_M.gguf",
            "D:/Model/translate/translategemma-4b-it.Q8_0.gguf",
            "D:/Model/translate/translategemma-12b-it-Q4_K_S.gguf",
            "D:/Model/translate/Index-Translate-9B.Q4_K_M.gguf",
        ] {
            let name = std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            match read_kv_shape(std::path::Path::new(p)) {
                Some(s) => println!(
                    "{name}  {s:?}  KV@4096={}MiB  KV@8192={}MiB",
                    s.kv_cache_bytes(4096) / (1024 * 1024),
                    s.kv_cache_bytes(8192) / (1024 * 1024),
                ),
                None => println!("{name}  -> None（缺字段，调用方应退回估算）"),
            }
        }
    }
}

#[cfg(test)]
mod kvshape_unit {
    use super::*;

    /// 普通模型：所有层都有 KV，公式与旧版一致。
    #[test]
    fn scalar_shape_matches_old_formula() {
        let s = KvShape::from_parts(32, "4", 128, 128, None, None).unwrap();
        assert_eq!(s.kv_heads, 4);
        assert_eq!(s.kv_head_layers, 32 * 4);
        assert_eq!(s.local_kv_head_layers, 0);
        // 32 层 × 4 头 × 256 维 × 4096 token × 1 字节 = 128 MiB
        assert_eq!(s.kv_cache_bytes(4096), 128 * 1024 * 1024);
    }

    /// 混合注意力数组：0 表示该层没有 KV，不能按全部层数算。
    #[test]
    fn array_shape_only_counts_layers_with_kv() {
        // 48 层，每 6 层一个全注意力层
        let mut list = vec!["0"; 48];
        for i in (0..48).step_by(6) {
            list[i] = "4";
        }
        let s = KvShape::from_parts(48, &list.join(","), 128, 128, None, None).unwrap();

        assert_eq!(s.kv_heads, 4);
        assert_eq!(s.kv_head_layers, 8 * 4);
        assert_eq!(s.local_kv_head_layers, 0);
        assert_eq!(s.kv_cache_bytes(4096), 8 * 4 * 256 * 4096);
    }

    /// 标量 + 全注意力间隔：全注意力层按 ctx，局部层按滑动窗口。
    #[test]
    fn interval_shape_uses_sliding_window_for_local_layers() {
        let s = KvShape::from_parts(48, "4", 128, 128, Some(6), Some(1024)).unwrap();

        assert_eq!(s.kv_head_layers, 8 * 4);
        assert_eq!(s.local_kv_head_layers, 40 * 4);
        assert_eq!(s.sliding_window, Some(1024));

        // 全上下文 8 层 + 局部 40 层 × min(4096, 1024)
        let expected = (8 * 4 * 4096 + 40 * 4 * 1024) * 256;
        assert_eq!(s.kv_cache_bytes(4096), expected);
    }

    /// 数组长度对不上层数时宁可放弃，不能错位计算。
    #[test]
    fn mismatched_array_length_is_rejected() {
        assert!(KvShape::from_parts(32, "4,0,4", 128, 128, None, None).is_none());
    }

    /// 所有层都没有 KV 的数组没有意义，返回 None。
    #[test]
    fn all_zero_array_is_rejected() {
        let zeros = vec!["0"; 32].join(",");
        assert!(KvShape::from_parts(32, &zeros, 128, 128, None, None).is_none());
    }

    /// 端到端读一份带 **数组** `head_count_kv` 的 GGUF 头。
    ///
    /// 这验证的是 `read_meta_many` 的数组分支，而不只是 `from_parts`。
    #[test]
    fn reads_array_metadata_from_gguf() {
        let mut list = vec![0u32; 48];
        for i in (0..48).step_by(6) {
            list[i] = 4;
        }

        let mut buf = Vec::new();
        write_header(&mut buf, 5);
        put_u32_kv(&mut buf, "test.block_count", 48);
        put_u32_array_kv(&mut buf, "test.attention.head_count_kv", &list);
        put_u32_kv(&mut buf, "test.attention.key_length", 128);
        put_u32_kv(&mut buf, "test.attention.value_length", 128);
        put_u32_kv(&mut buf, "test.full_attention_interval", 6);

        let path = write_temp_gguf(&buf);
        let s = read_kv_shape(&path).expect("应能读出数组元数据");
        assert_eq!(s.layers, 48);
        assert_eq!(s.kv_head_layers, 8 * 4);
        assert_eq!(s.local_kv_head_layers, 0);
        std::fs::remove_file(&path).ok();
    }

    /// 端到端读“标量 + 全注意力间隔 + 滑动窗口”。
    #[test]
    fn reads_interval_and_window_from_gguf() {
        let mut buf = Vec::new();
        write_header(&mut buf, 6);
        put_u32_kv(&mut buf, "test.block_count", 48);
        put_u32_kv(&mut buf, "test.attention.head_count_kv", 4);
        put_u32_kv(&mut buf, "test.attention.key_length", 128);
        put_u32_kv(&mut buf, "test.attention.value_length", 128);
        put_u32_kv(&mut buf, "test.full_attention_interval", 6);
        put_u32_kv(&mut buf, "test.attention.sliding_window", 1024);

        let path = write_temp_gguf(&buf);
        let s = read_kv_shape(&path).expect("应能读出间隔与窗口");
        assert_eq!(s.kv_head_layers, 8 * 4);
        assert_eq!(s.local_kv_head_layers, 40 * 4);
        assert_eq!(s.sliding_window, Some(1024));
        std::fs::remove_file(&path).ok();
    }

    fn write_header(buf: &mut Vec<u8>, kv_count: u64) {
        buf.extend_from_slice(b"GGUF");
        buf.extend_from_slice(&3u32.to_le_bytes());
        buf.extend_from_slice(&0u64.to_le_bytes());
        buf.extend_from_slice(&kv_count.to_le_bytes());
    }

    fn put_string(buf: &mut Vec<u8>, s: &str) {
        buf.extend_from_slice(&(s.len() as u64).to_le_bytes());
        buf.extend_from_slice(s.as_bytes());
    }

    fn put_u32_kv(buf: &mut Vec<u8>, key: &str, value: u32) {
        put_string(buf, key);
        buf.extend_from_slice(&4u32.to_le_bytes());
        buf.extend_from_slice(&value.to_le_bytes());
    }

    fn put_u32_array_kv(buf: &mut Vec<u8>, key: &str, values: &[u32]) {
        put_string(buf, key);
        buf.extend_from_slice(&9u32.to_le_bytes()); // ARRAY
        buf.extend_from_slice(&4u32.to_le_bytes()); // UINT32
        buf.extend_from_slice(&(values.len() as u64).to_le_bytes());
        for v in values {
            buf.extend_from_slice(&v.to_le_bytes());
        }
    }

    fn write_temp_gguf(buf: &[u8]) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("xieyi-kv-{nanos}.gguf"));
        std::fs::write(&path, buf).expect("写临时 GGUF");
        path
    }
}
