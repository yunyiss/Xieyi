//! 按模型保存的用户参数：上下文大小与思考开关。
//!
//! 这些参数写进 `presets.ini` / 请求体，**改上下文需要重启引擎**（llama.cpp 的
//! ctx 是进程级的），思考开关则是逐请求生效。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::AppHandle;

use crate::paths;

/// 新建模型的默认上下文。压到 4096 是为了别让默认 262144 吃爆显存。
pub const DEFAULT_CTX: u64 = 4096;
pub const MIN_CTX: u64 = 512;
pub const MAX_CTX: u64 = 262_144;

/// 思考档位。
///
/// 对齐 llama.cpp Web UI 的做法：`enable_thinking` 走对话模板，
/// `thinking_budget_tokens` 是**服务端硬限制**（已实测生效），
/// `reasoning_control` 表示由客户端掌控思考预算。
///
/// 注意：预算只管"思考什么时候收尾"，和 `max_tokens` 共用同一份输出额度。
/// 总预算太小的话，思考照样把正文挤掉，所以发预算时要同步放大 max_tokens。
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Thinking {
    /// 不干预，交给模型模板的默认行为
    Default,
    #[default]
    Off,
    Low,
    Medium,
    High,
    /// 不限思考长度
    Max,
}

impl Thinking {
    pub fn as_str(self) -> &'static str {
        match self {
            Thinking::Default => "default",
            Thinking::Off => "off",
            Thinking::Low => "low",
            Thinking::Medium => "medium",
            Thinking::High => "high",
            Thinking::Max => "max",
        }
    }

    /// 兼容旧配置：之前的 auto/on/off 三态。
    pub fn parse(s: &str) -> Self {
        match s {
            "auto" | "default" => Thinking::Default,
            "on" | "max" => Thinking::Max,
            "low" => Thinking::Low,
            "medium" => Thinking::Medium,
            "high" => Thinking::High,
            _ => Thinking::Off,
        }
    }

    /// 是否让模型思考。
    pub fn enabled(self) -> bool {
        matches!(self, Thinking::Low | Thinking::Medium | Thinking::High | Thinking::Max)
    }

    /// 思考预算（token）。`None` 表示不设置，交给服务端或模板。
    pub fn budget(self) -> Option<i64> {
        match self {
            Thinking::Low => Some(512),
            Thinking::Medium => Some(2048),
            Thinking::High => Some(8192),
            Thinking::Max => Some(-1),
            Thinking::Default | Thinking::Off => None,
        }
    }

    /// 思考最多会占多少 token（用于给 max_tokens 留余量）。
    /// 「不限」无法预估，单独在 [`Thinking::output_budget`] 里处理。
    pub fn reserved_tokens(self) -> u32 {
        match self {
            Thinking::Low => 512,
            Thinking::Medium => 2048,
            Thinking::High => 8192,
            Thinking::Default | Thinking::Off | Thinking::Max => 0,
        }
    }

    /// 这次请求的 `max_tokens`（正文与思考共用同一份额度）。
    ///
    /// 思考会把额度吃掉，所以：
    /// - 有限预算的档位：正文基准 + 该档思考预算
    /// - **不限档：直接把上下文的可用部分全给它**，否则思考必然把正文挤空
    /// - 关闭 / 默认：只留正文基准
    /// 最后统一夹到 `ctx / 2`，给提问内容留出另一半空间。
    pub fn output_budget(self, ctx: u64) -> u32 {
        let room = (ctx / 2).max(512);
        let want = match self {
            Thinking::Max => room,
            other => (BASE_OUTPUT_TOKENS as u64 + other.reserved_tokens() as u64).min(room),
        };
        want.max(1) as u32
    }
}

/// 正文本身的输出额度基准（不含思考）。前后端共用这一份。
pub const BASE_OUTPUT_TOKENS: u32 = 1024;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModelParams {
    #[serde(default = "default_ctx")]
    pub ctx: u64,
    #[serde(default)]
    pub thinking: Thinking,
}

fn default_ctx() -> u64 {
    DEFAULT_CTX
}

impl Default for ModelParams {
    fn default() -> Self {
        Self {
            ctx: DEFAULT_CTX,
            thinking: Thinking::Off,
        }
    }
}

/// 所有模型的参数（键是模型 ID）。
pub fn all(app: &AppHandle) -> HashMap<String, ModelParams> {
    paths::config_value(app, "modelParams")
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

pub fn get(app: &AppHandle, model: &str) -> ModelParams {
    all(app).remove(model).unwrap_or_default()
}

pub fn ctx_of(app: &AppHandle, model: &str) -> u64 {
    get(app, model).ctx
}

/// 写入单个模型的参数。上下文会被夹到合法区间，避免写进 0 或荒谬值。
pub fn set(app: &AppHandle, model: &str, params: &ModelParams) -> Result<ModelParams, String> {
    let normalized = ModelParams {
        ctx: params.ctx.clamp(MIN_CTX, MAX_CTX),
        thinking: params.thinking,
    };

    let mut map = all(app);
    map.insert(model.to_string(), normalized.clone());

    paths::set_config_value(
        app,
        "modelParams",
        serde_json::to_value(map).map_err(|e| e.to_string())?,
    )?;

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_round_trips_through_str() {
        for mode in [
            Thinking::Default,
            Thinking::Off,
            Thinking::Low,
            Thinking::Medium,
            Thinking::High,
            Thinking::Max,
        ] {
            assert_eq!(Thinking::parse(mode.as_str()), mode);
        }
    }

    #[test]
    fn legacy_thinking_values_still_load() {
        // 旧配置里是 auto/on/off
        assert_eq!(Thinking::parse("auto"), Thinking::Default);
        assert_eq!(Thinking::parse("on"), Thinking::Max);
        assert_eq!(Thinking::parse("off"), Thinking::Off);
    }

    #[test]
    fn unknown_thinking_defaults_to_off() {
        // 配置被手改坏时不能悄悄打开思考
        assert_eq!(Thinking::parse("bogus"), Thinking::Off);
    }

    #[test]
    fn budgets_match_llamacpp_web_ui() {
        // 与 llama.cpp Web UI 的 Low/Medium/High/Max 一一对应
        assert_eq!(Thinking::Low.budget(), Some(512));
        assert_eq!(Thinking::Medium.budget(), Some(2048));
        assert_eq!(Thinking::High.budget(), Some(8192));
        assert_eq!(Thinking::Max.budget(), Some(-1));
        assert_eq!(Thinking::Off.budget(), None);
        assert_eq!(Thinking::Default.budget(), None);
    }

    #[test]
    fn output_budget_fits_body_plus_thinking() {
        // 有限预算档：正文基准 + 思考预算，且不超过 ctx 的一半
        assert_eq!(Thinking::Off.output_budget(4096), 1024);
        assert_eq!(Thinking::Low.output_budget(4096), 1536);
        assert_eq!(Thinking::Medium.output_budget(8192), 3072);
        // ctx 太小时被 ctx/2 夹住
        assert_eq!(Thinking::High.output_budget(4096), 2048);
    }

    #[test]
    fn max_tier_gets_unlimited_room() {
        // 不限档必须拿到 ctx 的一半，否则思考会把正文挤空
        assert_eq!(Thinking::Max.output_budget(8192), 4096);
        assert_eq!(Thinking::Max.output_budget(32768), 16384);
        // 上下文极小时仍然给一个下限，不能是 0
        assert!(Thinking::Max.output_budget(512) >= 256);
    }

    #[test]
    fn budget_is_clamped_by_context_half() {
        // 每个有限预算档都有一个"刚好放得下"的上下文临界点：ctx/2 等于
        // 正文基准 + 思考预算。临界点之上不浪费额度，之下被 ctx/2 夹住。
        for mode in [Thinking::Low, Thinking::Medium, Thinking::High] {
            let want = BASE_OUTPUT_TOKENS as u64 + mode.reserved_tokens() as u64;
            let boundary = 2 * want;

            assert_eq!(mode.output_budget(boundary) as u64, want);
            assert_eq!(mode.output_budget(boundary + 100) as u64, want);
            // 差 1 就已经放不下，会被 ctx/2 夹小
            assert_eq!(mode.output_budget(boundary - 1) as u64, want - 1);
        }
    }

    #[test]
    fn tiers_without_thinking_keep_base_budget() {
        // 关闭 / 默认档只需要正文基准，且同样受 ctx/2 限制
        assert_eq!(Thinking::Off.output_budget(4096), BASE_OUTPUT_TOKENS);
        assert_eq!(Thinking::Default.output_budget(4096), BASE_OUTPUT_TOKENS);
        assert_eq!(Thinking::Off.output_budget(1024), 512);
    }

    #[test]
    fn enabled_only_for_thinking_tiers() {
        assert!(!Thinking::Off.enabled());
        assert!(!Thinking::Default.enabled());
        for mode in [Thinking::Low, Thinking::Medium, Thinking::High, Thinking::Max] {
            assert!(mode.enabled(), "{mode:?} 应被视为开启思考");
        }
    }

    #[test]
    fn ctx_is_clamped_to_sane_range() {
        let low = ModelParams { ctx: 1, thinking: Thinking::Off };
        assert_eq!(low.ctx.clamp(MIN_CTX, MAX_CTX), MIN_CTX);
        let high = ModelParams { ctx: u64::MAX, thinking: Thinking::Off };
        assert_eq!(high.ctx.clamp(MIN_CTX, MAX_CTX), MAX_CTX);
    }

    #[test]
    fn default_params_are_4096_and_no_thinking() {
        let p = ModelParams::default();
        assert_eq!(p.ctx, DEFAULT_CTX);
        assert_eq!(p.thinking, Thinking::Off);
    }
}