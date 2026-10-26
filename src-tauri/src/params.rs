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

/// 思考开关。
///
/// 能否真正生效取决于模型自己的对话模板：实测 Qwen3.5 只认 `enable_thinking`
/// 这个布尔值，没有预算/强度参数；Hy-MT2 这类翻译模板会直接忽略它。
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Thinking {
    /// 不干预，交给模板默认行为（思考型模型会先输出思维链）
    Auto,
    On,
    #[default]
    Off,
}

impl Thinking {
    pub fn as_str(self) -> &'static str {
        match self {
            Thinking::Auto => "auto",
            Thinking::On => "on",
            Thinking::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "auto" => Thinking::Auto,
            "on" => Thinking::On,
            _ => Thinking::Off,
        }
    }
}

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
        for mode in [Thinking::Auto, Thinking::On, Thinking::Off] {
            assert_eq!(Thinking::parse(mode.as_str()), mode);
        }
    }

    #[test]
    fn unknown_thinking_defaults_to_off() {
        // 配置被手改坏时不能悄悄打开思考
        assert_eq!(Thinking::parse("bogus"), Thinking::Off);
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