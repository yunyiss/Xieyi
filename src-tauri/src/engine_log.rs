//! 解析 llama-server 子进程日志，提取“加载到哪一步”和“实际用了多少资源”。
//!
//! ## 为什么需要它
//!
//! 路由模式（`--models-max 1`）下，模型是引擎自己加载的；我们只能看到
//! `/models` 的 `loading/loaded` 两个状态，中间是一段黑盒。用户在这段时间
//! 里唯一能看到的是一句“加载中”，分不清是在读盘、分配缓存，还是卡死了。
//!
//! llama.cpp 在 `-lv 4` 下会打印足够细的日志：
//!
//! ```text
//! load_model: loading model '...'
//! load_tensors: loading model tensors, this can take a while...
//! load_tensors: offloaded 33/33 layers to GPU
//! load_tensors:      Vulkan0 model buffer size =  1815.26 MiB
//! llama_context: n_ctx                 = 4096
//! llama_kv_cache:    Vulkan0 KV buffer size =    17.00 MiB
//! llama_server: model loaded
//! ```
//!
//! 本模块**只做纯解析**：输入一行日志，更新 [`LoadInfo`]。不读文件、
//! 不碰 AppHandle，方便用真实日志片段做单测。
//!
//! ## 兼容性
//!
//! 老版本引擎可能没有 `log_verbosity` 或日志格式不同。解析不到就保留
//! `None`，上层退回原来的“按体积估算”进度，不会因为解析失败而不干活。

/// 加载阶段。数值顺序用于“只前进、不回退”。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum LoadStage {
    /// 已发起加载，引擎还没开始读模型。
    #[default]
    Prepare,
    /// 正在读取 GGUF / 装填权重。
    Read,
    /// 权重已就位，正在分配 KV cache / 计算缓冲。
    Cache,
    /// 引擎报告 model loaded。
    Ready,
}

impl LoadStage {
    /// 给前端的状态码，避免前端解析中文。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Read => "read",
            Self::Cache => "cache",
            Self::Ready => "ready",
        }
    }

    fn advance(&mut self, next: Self) -> bool {
        if next > *self {
            *self = next;
            true
        } else {
            false
        }
    }
}

/// 从日志里累积出来的加载信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadInfo {
    pub stage: LoadStage,
    /// 实际放到显卡上的层数（`load_tensors: offloaded N/M layers` 的 N）。
    pub gpu_layers: Option<u32>,
    /// 模型总层数（同上 M）。
    pub total_layers: Option<u32>,
    /// KV cache 实际占用（字节）。
    pub kv_bytes: Option<u64>,
    /// 实际上下文长度。
    pub ctx: Option<u64>,
    /// 显卡上的模型权重缓冲（字节）。
    pub gpu_model_bytes: Option<u64>,
    /// 内存里的模型权重缓冲（字节）。
    pub cpu_model_bytes: Option<u64>,
    /// 引擎预计要占用的显存（字节）。
    pub projected_bytes: Option<u64>,
    /// 加载前显卡空闲显存（字节）。
    pub free_bytes: Option<u64>,
}

impl LoadInfo {
    /// 是否至少拿到了“实际分配”里最关键的两项。
    pub fn has_placement(&self) -> bool {
        self.gpu_layers.is_some() && self.total_layers.is_some()
    }
}

/// 解析一行日志；有字段变化时返回 `true`。
pub fn parse_line(info: &mut LoadInfo, line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() {
        return false;
    }
    let mut changed = false;

    // 阶段推进
    if line.contains("load_model: loading model")
        || line.contains("load_tensors: loading model tensors")
    {
        changed |= info.stage.advance(LoadStage::Read);
    }
    if line.contains("load_model: initializing,")
        || line.contains("llama_context: constructing llama_context")
    {
        changed |= info.stage.advance(LoadStage::Cache);
    }
    if line.contains("llama_server: model loaded") {
        changed |= info.stage.advance(LoadStage::Ready);
    }

    // 实际层数：offloaded 33/33 layers to GPU
    if let Some((gpu, total)) = parse_offloaded_layers(line) {
        if info.gpu_layers != Some(gpu) || info.total_layers != Some(total) {
            info.gpu_layers = Some(gpu);
            info.total_layers = Some(total);
            changed = true;
        }
        // 层数行出现在权重装填结束时，后面就是分配上下文缓存。
        changed |= info.stage.advance(LoadStage::Cache);
    }

    // 实际上下文：llama_context: n_ctx = 4096（不要误匹配 n_ctx_seq / n_ctx_train）
    if let Some(ctx) = parse_n_ctx(line)
        && info.ctx != Some(ctx)
    {
        info.ctx = Some(ctx);
        changed = true;
    }

    // KV cache 实际占用
    if line.contains("llama_kv_cache:")
        && line.contains("KV buffer size")
        && let Some(bytes) = parse_mib_after(line, "KV buffer size")
        && info.kv_bytes != Some(bytes)
    {
        info.kv_bytes = Some(bytes);
        changed = true;
    }

    // 模型权重缓冲：CPU_Mapped / Vulkan0 / CUDA0 / Metal ...
    if line.contains("load_tensors:")
        && line.contains("model buffer size")
        && let (Some(device), Some(bytes)) = (
            device_after_load_tensors(line),
            parse_mib_after(line, "model buffer size"),
        )
    {
        let slot = if device.starts_with("CPU") {
            &mut info.cpu_model_bytes
        } else {
            &mut info.gpu_model_bytes
        };
        if *slot != Some(bytes) {
            *slot = Some(bytes);
            changed = true;
        }
    }

    // 引擎预计占用 / 加载前空闲：projected to use 1884 MiB ... vs. 7186 MiB ...
    if line.contains("projected to use ")
        && let Some((projected, free)) = parse_projected(line)
        && (info.projected_bytes != Some(projected) || info.free_bytes != Some(free))
    {
        info.projected_bytes = Some(projected);
        info.free_bytes = Some(free);
        changed = true;
    }

    changed
}

/// `offloaded 33/33 layers to GPU` → `(33, 33)`
fn parse_offloaded_layers(line: &str) -> Option<(u32, u32)> {
    let rest = line.split("offloaded ").nth(1)?;
    let pair = rest.split_whitespace().next()?;
    let mut it = pair.split('/');
    let gpu = it.next()?.parse().ok()?;
    let total = it.next()?.parse().ok()?;
    Some((gpu, total))
}

/// `llama_context: n_ctx                 = 4096`
fn parse_n_ctx(line: &str) -> Option<u64> {
    let marker = "llama_context: n_ctx";
    let pos = line.find(marker)?;
    let rest = &line[pos + marker.len()..];
    // 必须紧跟空白，排除 n_ctx_seq / n_ctx_train
    if !rest.chars().next().is_some_and(|c| c.is_whitespace()) {
        return None;
    }
    let after_eq = rest.split('=').nth(1)?;
    after_eq.split_whitespace().next()?.parse().ok()
}

/// 取 `load_tensors:` 后面的设备名（CPU_Mapped / Vulkan0 / CUDA0 ...）
fn device_after_load_tensors(line: &str) -> Option<&str> {
    let pos = line.find("load_tensors:")?;
    let rest = line[pos + "load_tensors:".len()..].trim_start();
    rest.split_whitespace().next()
}

/// 取 `marker` 后面第一个数字，按 MiB 转成字节。
fn parse_mib_after(line: &str, marker: &str) -> Option<u64> {
    let pos = line.find(marker)?;
    let rest = &line[pos + marker.len()..];
    let value = rest
        .split_whitespace()
        .find_map(|token| token.parse::<f64>().ok())?;
    Some((value * 1024.0 * 1024.0).round() as u64)
}

/// `projected to use 1884 MiB ... vs. 7186 MiB ...` → (1884MiB, 7186MiB)
fn parse_projected(line: &str) -> Option<(u64, u64)> {
    let rest = line.split("projected to use ").nth(1)?;
    let projected = rest.split_whitespace().next()?.parse::<f64>().ok()?;
    let tail = rest.split("vs.").nth(1)?;
    let free = tail
        .split_whitespace()
        .find_map(|token| token.parse::<f64>().ok())?;
    Some((
        (projected * 1024.0 * 1024.0).round() as u64,
        (free * 1024.0 * 1024.0).round() as u64,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实日志（b11714，Hy-MT2-1.8B，Vulkan，q8_0 KV，ctx=512）。
    const REAL: &[&str] = &[
        "0.00.203.168 I srv    load_model: loading model 'D:\\Model\\translate\\Hy-MT2-1.8B-Q8_0.gguf'",
        "0.00.476.148 I llama_model_loader: loaded meta data with 32 key-value pairs and 354 tensors",
        "0.00.606.297 I load_tensors: loading model tensors, this can take a while... (load_mode = mmap)",
        "0.00.735.618 I load_tensors: offloading 31 repeating layers to GPU",
        "0.00.735.618 I load_tensors: offloaded 33/33 layers to GPU",
        "0.00.735.623 I load_tensors:   CPU_Mapped model buffer size =   250.72 MiB",
        "0.00.735.624 I load_tensors:      Vulkan0 model buffer size =  1815.26 MiB",
        "0.01.699.806 I llama_context: n_ctx                 = 512",
        "0.01.700.895 I llama_kv_cache:    Vulkan0 KV buffer size =    17.00 MiB",
        "0.02.094.743 I srv  llama_server: model loaded",
    ];

    #[test]
    fn parses_real_load_sequence() {
        let mut info = LoadInfo::default();
        for line in REAL {
            parse_line(&mut info, line);
        }

        assert_eq!(info.stage, LoadStage::Ready);
        assert_eq!(info.gpu_layers, Some(33));
        assert_eq!(info.total_layers, Some(33));
        assert_eq!(info.ctx, Some(512));
        assert_eq!(info.kv_bytes, Some(17 * 1024 * 1024));
        assert_eq!(
            info.gpu_model_bytes,
            Some((1815.26 * 1024.0 * 1024.0_f64).round() as u64)
        );
        assert_eq!(
            info.cpu_model_bytes,
            Some((250.72 * 1024.0 * 1024.0_f64).round() as u64)
        );
        assert!(info.has_placement());
    }

    #[test]
    fn stage_only_moves_forward() {
        let mut info = LoadInfo::default();
        parse_line(&mut info, "llama_server: model loaded");
        assert_eq!(info.stage, LoadStage::Ready);
        // 后到的读取日志不能把阶段拉回去
        parse_line(&mut info, "load_model: loading model 'x.gguf'");
        assert_eq!(info.stage, LoadStage::Ready);
    }

    #[test]
    fn n_ctx_ignores_seq_and_train() {
        let mut info = LoadInfo::default();
        assert!(!parse_line(
            &mut info,
            "llama_context: n_ctx_seq (512) < n_ctx_train (262144)"
        ));
        assert_eq!(info.ctx, None);
        assert!(parse_line(
            &mut info,
            "llama_context: n_ctx                 = 4096"
        ));
        assert_eq!(info.ctx, Some(4096));
    }

    #[test]
    fn parses_projected_and_free() {
        let mut info = LoadInfo::default();
        parse_line(
            &mut info,
            "0.00.452.705 I common_params_fit_impl: projected to use 1899 MiB of device memory vs. 7186 MiB of free device memory",
        );
        assert_eq!(
            info.projected_bytes,
            Some((1899.0 * 1024.0 * 1024.0_f64).round() as u64)
        );
        assert_eq!(
            info.free_bytes,
            Some((7186.0 * 1024.0 * 1024.0_f64).round() as u64)
        );
    }

    #[test]
    fn unknown_lines_do_not_change_anything() {
        let mut info = LoadInfo::default();
        assert!(!parse_line(
            &mut info,
            "0.00.1 I srv update_slots: all slots are idle"
        ));
        assert_eq!(info, LoadInfo::default());
    }
}
