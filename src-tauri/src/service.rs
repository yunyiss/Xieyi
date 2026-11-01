use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use crate::models;
use crate::params::{self, Thinking};
use crate::paths;
use crate::prompts::{self, Style};
use crate::quality;
use crate::runtime;
use crate::translate as segmenter;

pub struct Service {
    pub port: u16,
    pub key: String,
    child: tokio::sync::Mutex<tokio::process::Child>,
    loaded: tokio::sync::Mutex<Vec<String>>,
    client: reqwest::Client,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub loaded: Vec<String>,
    pub models_dir: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RouterModel {
    pub id: String,
    pub state: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TokenPayload {
    request_id: String,
    token: String,
}

/// 一次补全的 token 用量，用于在界面上显示"上下文用了多少"。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UsagePayload {
    request_id: String,
    /// 本次输入占用（含系统提示与历史）
    prompt_tokens: u64,
    /// 本次输出占用
    completion_tokens: u64,
    /// 命中缓存、无需重算的前缀 token 数
    cached_tokens: u64,
    /// 该模型当前的上下文上限
    ctx_size: u64,
    /// 本次请求耗时（毫秒），用于算 tokens/s
    elapsed_ms: u64,
}

/// 质检报告：某个片段翻完之后被查出了问题（可能已重试修好）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct QualityPayload {
    request_id: String,
    /// 出问题那段的开头几个字，方便定位
    segment: String,
    /// 检出了什么
    found: String,
    /// 重试后是否修好
    fixed: bool,
}

/// 单个片段的质量统计，供批量任务汇总"检出多少、修好多少"。
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SegmentStats {
    /// 首次输出被质检挑出了问题（无论最后有没有修好）
    pub flagged: bool,
    /// 重试后的结果被采用，并且通过了质检
    pub fixed: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ModelStatePayload {
    model: String,
    state: String,
    /// 加载阶段的可读描述（如「装填权重」「预热」）。
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    estimate_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    percent: Option<u8>,
}

impl ModelStatePayload {
    fn simple(model: &str, state: &str) -> Self {
        Self {
            model: model.to_string(),
            state: state.to_string(),
            phase: None,
            elapsed_ms: None,
            estimate_ms: None,
            percent: None,
        }
    }

    /// 加载中：llama.cpp 不提供真实进度，这里按体积估算并附带已耗时。
    fn loading(model: &str, elapsed_ms: u64, estimate_ms: u64) -> Self {
        let ratio = elapsed_ms as f64 / estimate_ms.max(1) as f64;
        let percent = (ratio * 92.0).clamp(0.0, 92.0) as u8;

        Self {
            model: model.to_string(),
            state: "loading".into(),
            phase: Some("装填权重".into()),
            elapsed_ms: Some(elapsed_ms),
            estimate_ms: Some(estimate_ms),
            percent: Some(percent),
        }
    }

    /// 预热中：首推理开销（Vulkan 着色器编译约 3 秒）已挪到这一阶段。
    fn warming(model: &str, estimate_ms: u64) -> Self {
        Self {
            model: model.to_string(),
            state: "warming".into(),
            phase: Some("预热".into()),
            elapsed_ms: Some(0),
            estimate_ms: Some(estimate_ms),
            percent: Some(94),
        }
    }
}

/// 按模型体积估算加载耗时。实测基线：1.1GB ≈ 2.5s，5.5GB ≈ 7s。
/// 仅用于展示预计剩余，不是精确值。
fn estimate_load_ms(size_mb: u64) -> u64 {
    1200 + size_mb * 12 / 10
}

/// 从本地模型目录查体积；只读文件元数据，开销很小。
fn model_size_mb(app: &AppHandle, model: &str) -> u64 {
    models::scan(app)
        .into_iter()
        .find(|m| m.id == model)
        .map(|m| m.size_mb)
        .unwrap_or(0)
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryEntry {
    pub from: String,
    pub to: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TranslateProgress {
    done: usize,
    total: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslateRequest {
    #[serde(default)]
    pub request_id: String,
    pub model: String,
    pub source: String,
    pub target: String,
    /// 翻译风格（general/formal/technical/literary/subtitle），空值按 general。
    #[serde(default)]
    pub style: String,
    pub text: String,
}

pub struct AppState {
    pub service: tokio::sync::Mutex<Option<Arc<Service>>>,
    /// 串行化推理请求，避免同时向单模型服务打并发；不阻塞状态查询。
    pub infer_lock: tokio::sync::Mutex<()>,
    /// 正在进行/排队中的推理请求数。
    ///
    /// TTL 的本意是"用户走开了就把显存还回去"，不能用它给**正在使用**的请求计时：
    /// 思考型模型一次生成可能好几分钟，期间 `last_used` 不动，空闲计时照样会
    /// 走满并把模型卸掉——表现为思考到一半突然中断、只留下一小段内容。
    /// 因此只要这个计数大于 0，TTL 就必须让路。
    pub inflight: Arc<AtomicU64>,
    /// 当前请求是否被用户取消。
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    pub last_used: tokio::sync::Mutex<Instant>,
    pub ttl_secs: AtomicU64,
    pub gpu_layers: AtomicU64,
    /// 常驻（Pinned）模型：永不自动卸载。
    pub pinned: tokio::sync::Mutex<HashSet<String>>,
    /// 句子级翻译缓存：相同模型 / 风格 / 语言对 / 术语表 / 原文直接命中。
    /// 值里带上当时的质检统计，命中缓存时批量汇总仍然准确。
    pub cache: tokio::sync::Mutex<HashMap<String, (String, SegmentStats)>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            service: tokio::sync::Mutex::new(None),
            infer_lock: tokio::sync::Mutex::new(()),
            inflight: Arc::new(AtomicU64::new(0)),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_used: tokio::sync::Mutex::new(Instant::now()),
            ttl_secs: AtomicU64::new(120),
            gpu_layers: AtomicU64::new(models::AUTO_LAYERS),
            pinned: tokio::sync::Mutex::new(HashSet::new()),
            cache: tokio::sync::Mutex::new(HashMap::new()),
        }
    }
}

/// 翻译缓存上限。超过后整体清空——翻译多是一次性内容，
/// 做 LRU 的复杂度换不来收益。
const CACHE_LIMIT: usize = 4000;

/// 推理期间给 TTL 让路的守卫：构造时 +1，Drop 时 -1。
///
/// 用 RAII 而不是"在函数末尾手动减"：这条路径上有大量 `?` 提前返回、
/// 还有取消/超时分支，手写递减迟早漏一处，而漏掉的后果很严重——
/// 计数永远大于 0，TTL 从此彻底失效，显存再也不释放。
pub(crate) struct InflightGuard(Arc<AtomicU64>);

impl InflightGuard {
    pub(crate) fn new(counter: &Arc<AtomicU64>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter.clone())
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// 把一段译文写进缓存，**唯一的写入口**。
///
/// 被取消的片段一律不写：`stream_completion` 在取消时会把已经流出的那半截
/// 当作正常结果返回（界面要保留已显示内容，不能凭空清空），但那不是一段翻完的
/// 译文。写进去以后，同样的原文会直接命中它，再无机会重新翻译——用户会拿到一段
/// 看着正常、实际被截断的译文，而且查不出原因。
fn store_segment(
    cache: &mut HashMap<String, (String, SegmentStats)>,
    key: String,
    text: String,
    stats: SegmentStats,
    cancelled: bool,
) {
    if cancelled {
        return;
    }
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, (text, stats));
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub ttl_secs: u64,
    pub gpu_layers: u64,
    pub pinned: Vec<String>,
    pub models_dir: String,
    pub running: bool,
    pub port: Option<u16>,
}

/// 启动时把持久化设置装回内存。
pub fn load_settings(app: &AppHandle) {
    let state = app.state::<AppState>();

    if let Some(ttl) = paths::config_value(app, "ttlSecs").and_then(|v| v.as_u64()) {
        state.ttl_secs.store(ttl, Ordering::Relaxed);
    }
    if let Some(layers) = paths::config_value(app, "gpuLayers").and_then(|v| v.as_u64()) {
        state.gpu_layers.store(layers, Ordering::Relaxed);
    }
    drop(state);

    if let Some(list) = paths::config_value(app, "pinned").and_then(|v| v.as_array().cloned()) {
        let set: HashSet<String> = list
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        if !set.is_empty() {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<AppState>();
                *state.pinned.lock().await = set;
            });
        }
    }
}

fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("分配端口失败: {e}"))?;
    listener.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

fn local_key() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{n:x}{:x}", std::process::id())
}

impl Service {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    pub async fn is_alive(&self) -> bool {
        let mut child = self.child.lock().await;
        child.try_wait().ok().flatten().is_none()
    }

    async fn get_models(&self) -> Result<Vec<RouterModel>, String> {
        let resp = self
            .client
            .get(self.url("/models"))
            .bearer_auth(&self.key)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| format!("查询模型列表失败: {e}"))?;

        let value: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        if let Some(list) = value["data"].as_array() {
            for item in list {
                out.push(RouterModel {
                    id: item["id"].as_str().unwrap_or_default().to_string(),
                    state: item["status"]["value"].as_str().unwrap_or("unknown").to_string(),
                });
            }
        }
        Ok(out)
    }

    async fn post_json(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let resp = self
            .client
            .post(self.url(path))
            .bearer_auth(&self.key)
            .json(&body)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("{path} 请求失败: {e}"))?;
        let value: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        Ok(value)
    }

    pub async fn state_of(&self, model: &str) -> Result<String, String> {
        Ok(self
            .get_models()
            .await?
            .into_iter()
            .find(|m| m.id == model)
            .map(|m| m.state)
            .unwrap_or_else(|| "missing".into()))
    }

    pub async fn loaded_ids(&self) -> Vec<String> {
        self.loaded.lock().await.clone()
    }

    /// `/models/load` 是异步的：必须轮询到 loaded 才算就绪。
    /// `cancel` 为真时立即中止等待，让调用方可以放弃一次慢加载。
    async fn wait_state(
        &self,
        model: &str,
        want: &str,
        timeout: Duration,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<(), String> {
        let started = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("已取消".into());
            }
            let state = self.state_of(model).await?;
            if state == want {
                return Ok(());
            }
            if started.elapsed() > timeout {
                return Err(format!("等待模型状态 {want} 超时（当前 {state}）"));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// 加载专用等待：除了等 loaded，还要识别"加载中途回到 unloaded"
    /// —— 这通常意味着该 GGUF 不被当前 llama.cpp 构建支持，会立刻失败而不是干等超时。
    ///
    /// 期间按 500ms 节流上报 loading 状态（已耗时 + 估算总时长），
    /// 让前端能显示进度而不是一片空白。这里的百分比是按体积估算的，不等于真实进度。
    async fn wait_loaded(
        &self,
        app: &AppHandle,
        model: &str,
        timeout: Duration,
        cancel: &std::sync::atomic::AtomicBool,
        estimate_ms: u64,
    ) -> Result<(), String> {
        let started = Instant::now();
        let mut seen_loading = false;
        let mut last_emit = Duration::ZERO;

        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("已取消".into());
            }

            let state = self.state_of(model).await?;
            match state.as_str() {
                "loaded" => return Ok(()),
                "loading" => {
                    seen_loading = true;
                    let elapsed = started.elapsed();
                    if elapsed.saturating_sub(last_emit) >= Duration::from_millis(500) {
                        last_emit = elapsed;
                        let _ = app.emit(
                            "model://state-changed",
                            ModelStatePayload::loading(
                                model,
                                elapsed.as_millis() as u64,
                                estimate_ms,
                            ),
                        );
                    }
                }
                "unloaded" | "failed" if seen_loading => {
                    return Err(format!(
                        "模型加载失败（{model}）。可能是该 GGUF 的量化格式不受当前 llama.cpp 版本支持，或显存不足。"
                    ))
                }
                _ => {}
            }

            if started.elapsed() > timeout {
                return Err(format!("模型加载超时（当前状态 {state}）"));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// 预热：跑一次极短推理，把首推理开销（Vulkan 着色器编译约 3 秒）
    /// 从"用户看到的第一句话"挪到"加载阶段"。失败不影响使用，直接忽略。
    async fn warmup(&self, model: &str, cancel: &std::sync::atomic::AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }

        let body = serde_json::json!({
            "model": model,
            "messages": [{ "role": "user", "content": "hi" }],
            "max_tokens": 1,
            "stream": false,
            "chat_template_kwargs": { "enable_thinking": false }
        });

        self.client
            .post(self.url("/v1/chat/completions"))
            .bearer_auth(&self.key)
            .json(&body)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| format!("预热失败: {e}"))?;
        Ok(())
    }

    /// 翻译单个片段：命中缓存直接返回，否则流式推理、质检、必要时重试一次，再回填缓存。
    ///
    /// `request_id` 为 `None` 时不会向前端推送 token —— 批量任务走这条路径，
    /// 避免把批量过程的中间结果灌进翻译页的输出框。
    ///
    /// 返回值带上质检统计：批量任务据此汇总"检出多少段、修好多少段"。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn translate_segment(
        &self,
        app: &AppHandle,
        state: &AppState,
        request_id: Option<&str>,
        model: &str,
        source: &str,
        target: &str,
        style: Style,
        glossary: &[GlossaryEntry],
        text: &str,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<(String, SegmentStats), String> {
        let key = cache_key(style, model, source, target, glossary, text);

        if let Some((hit, stats)) = state.cache.lock().await.get(&key).cloned() {
            if let Some(id) = request_id {
                let _ = app.emit(
                    "llm://token",
                    TokenPayload {
                        request_id: id.to_string(),
                        token: hit.clone(),
                    },
                );
            }
            // 缓存里的结果在写入前已经过了质检，不必再查一次；
            // 统计沿用首次生成时的结果，重复跑批量不会把"检出的段"算丢。
            return Ok((hit, stats));
        }

        let terms: Vec<quality::Term> = glossary
            .iter()
            .map(|e| quality::Term {
                from: &e.from,
                to: &e.to,
            })
            .collect();

        // 按模型的调用机制生成请求：不同翻译模型的接口差异很大，
        // 用错不一定报错，而是静默给出错误结果（如把一切翻成英语）。
        // 认不出来的模型走 EnglishSystem，与改造前逐字一致。
        let profile = model_profile(app, model);
        // 用错调用机制不会报错、只会静默给出错误结果，所以留一行痕迹方便排查。
        // 只在机制变化时打印：批量翻译有几十上百段，逐段刷屏没有意义。
        log_profile_once(model, &profile);
        let request = prompts::build_request(
            profile.style,
            style,
            source,
            target,
            glossary,
            text,
            None,
        );

        let first = stream_completion_with(
            app,
            self,
            request_id.unwrap_or_default(),
            model,
            request.messages,
            0.2,
            1024,
            // 翻译不让模型思考：既慢又容易把输出预算吃光
            Thinking::Off,
            request.template_kwargs,
            cancel,
        )
        .await?;

        let report = quality::check(text, &first, &terms, target);
        // 首次检出的问题（用户看到的是"发现了什么"，不是重试之后的状态）
        let found = report.describe();
        let flagged = !report.is_ok();
        let mut best = first;
        let mut best_report = report;
        // 只有"重试结果被采用且通过质检"才算修好
        let mut fixed = false;

        // 有问题就带纠正提示重试一次。重试**不推流**：第一次的内容已经流到界面上了，
        // 再流一遍会在输出框里重复一遍；翻译页最后会用返回值整体覆盖显示。
        // 已经被取消就不重试了，那只会白跑一次请求。
        if !best_report.is_ok() && !cancel.load(Ordering::Relaxed) {
            // 重试沿用同一个模型的调用机制，只把纠正提示塞进对应位置。
            // 提示词的语言要跟模型匹配：中文模板模型收到英文纠正会把它当正文翻译。
            let hint = best_report.retry_hint(prompts::hint_lang_for(profile.style));
            let retry = prompts::build_request(
                profile.style,
                style,
                source,
                target,
                glossary,
                text,
                hint.as_deref(),
            );
            let second = stream_completion_with(
                app,
                self,
                "",
                model,
                retry.messages,
                0.2,
                1024,
                Thinking::Off,
                retry.template_kwargs,
                cancel,
            )
            .await;

            if let Ok(second) = second {
                let second_report = quality::check(text, &second, &terms, target);
                // 只有确实更干净（或同样干净但更完整）才采用，避免越改越糟
                if quality::better_than(&second, &second_report, &best, &best_report) {
                    best = second;
                    best_report = second_report;
                    fixed = best_report.is_ok();
                }
            }

            let _ = app.emit(
                "translate://quality",
                QualityPayload {
                    request_id: request_id.unwrap_or_default().to_string(),
                    segment: text.chars().take(24).collect(),
                    found,
                    fixed,
                },
            );
        }

        let stats = SegmentStats { flagged, fixed };
        let cancelled = cancel.load(Ordering::Relaxed);

        {
            let mut cache = state.cache.lock().await;
            store_segment(&mut cache, key, best.clone(), stats, cancelled);
        }

        Ok((best, stats))
    }

    pub async fn load(
        &self,
        app: &AppHandle,
        model: &str,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<(), String> {
        if self.state_of(model).await? == "loaded" {
            let mut loaded = self.loaded.lock().await;
            if !loaded.iter().any(|m| m == model) {
                loaded.push(model.to_string());
            }
            return Ok(());
        }

        let estimate_ms = estimate_load_ms(model_size_mb(app, model));
        let _ = app.emit(
            "model://state-changed",
            ModelStatePayload::loading(model, 0, estimate_ms),
        );
        self.post_json("/models/load", serde_json::json!({ "model": model }))
            .await?;

        let outcome = self
            .wait_loaded(app, model, Duration::from_secs(180), cancel, estimate_ms)
            .await;

        if outcome.is_err() {
            // 取消或超时：把半加载的实例卸掉，避免显存被占住
            let _ = self
                .post_json("/models/unload", serde_json::json!({ "model": model }))
                .await;
            self.loaded.lock().await.retain(|m| m != model);
            let _ = app.emit(
                "model://state-changed",
                ModelStatePayload::simple(model, "cold"),
            );
            return outcome;
        }

        // 权重已就位，接着把首推理开销消化掉，用户的第一句话就不会卡在着色器编译上
        let _ = app.emit(
            "model://state-changed",
            ModelStatePayload::warming(model, estimate_ms),
        );
        let _ = self.warmup(model, cancel).await;

        let mut loaded = self.loaded.lock().await;
        if !loaded.iter().any(|m| m == model) {
            loaded.push(model.to_string());
        }
        drop(loaded);

        let _ = app.emit(
            "model://state-changed",
            ModelStatePayload::simple(model, "warm"),
        );
        Ok(())
    }

    pub async fn unload(&self, app: &AppHandle, model: &str) -> Result<(), String> {
        let _ = app.emit(
            "model://state-changed",
            ModelStatePayload::simple(model, "unloading"),
        );
        let no_cancel = std::sync::atomic::AtomicBool::new(false);
        self.post_json("/models/unload", serde_json::json!({ "model": model }))
            .await?;
        let _ = self
            .wait_state(model, "unloaded", Duration::from_secs(60), &no_cancel)
            .await;
        self.loaded.lock().await.retain(|m| m != model);
        let _ = app.emit(
            "model://state-changed",
            ModelStatePayload::simple(model, "cold"),
        );
        Ok(())
    }

    pub async fn kill(&self) {
        let mut child = self.child.lock().await;
        if let Some(pid) = child.id() {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .creation_flags(0x0800_0000)
                    .status();
            }
            #[cfg(not(windows))]
            {
                let _ = std::process::Command::new("kill")
                    .args(["-9", &pid.to_string()])
                    .status();
            }
        }
        let _ = child.start_kill();
    }
}

async fn spawn(app: &AppHandle, gpu_layers: u64) -> Result<Service, String> {
    let exe = runtime::server_exe(app)?;
    let models_dir = paths::models_dir(app)?;
    paths::ensure(&models_dir)?;

    let files = models::scan(app);
    if files.is_empty() {
        return Err(format!(
            "模型目录为空：{}。请放入 .gguf 模型文件后重试。",
            models_dir.display()
        ));
    }
    let preset = models::write_preset(app, &files, gpu_layers)?;

    let port = free_port()?;
    let key = local_key();

    let mut cmd = tokio::process::Command::new(&exe);
    cmd.args([
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--models-dir",
        &models_dir.to_string_lossy(),
        "--models-preset",
        &preset.to_string_lossy(),
        "--models-max",
        "1",
        "--no-models-autoload",
        "--jinja",
        "--no-warmup",
        "--api-key",
        &key,
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .kill_on_drop(true);

    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let child = cmd.spawn().map_err(|e| format!("启动推理引擎失败: {e}"))?;

    let service = Service {
        child: tokio::sync::Mutex::new(child),
        port,
        key,
        loaded: tokio::sync::Mutex::new(Vec::new()),
        client: reqwest::Client::new(),
    };

    // 等待服务就绪
    let started = Instant::now();
    loop {
        let resp = service
            .client
            .get(service.url("/health"))
            .bearer_auth(&service.key)
            .timeout(Duration::from_secs(2))
            .send()
            .await;
        if let Ok(r) = resp {
            if r.status().is_success() {
                break;
            }
        }
        if started.elapsed() > Duration::from_secs(30) {
            service.kill().await;
            return Err("推理引擎启动超时".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    Ok(service)
}

/// 确保引擎在跑，并返回共享句柄。
/// 只在启动时持锁，推理期间不再占着全局锁，状态查询不会被阻塞。
pub async fn ensure(app: &AppHandle, state: &AppState) -> Result<Arc<Service>, String> {
    let mut guard = state.service.lock().await;

    if let Some(existing) = guard.as_ref() {
        if existing.is_alive().await {
            return Ok(existing.clone());
        }
    }

    if let Some(old) = guard.take() {
        old.kill().await;
    }
    let layers = state.gpu_layers.load(Ordering::Relaxed);
    let service = Arc::new(spawn(app, layers).await?);
    *guard = Some(service.clone());
    Ok(service)
}

/// 只读取当前服务句柄，不触发启动。
pub async fn current(state: &AppState) -> Option<Arc<Service>> {
    state.service.lock().await.clone()
}

#[tauri::command]
pub async fn service_status(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<ServiceStatus, String> {
    let models_dir = paths::models_dir(&app)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    match current(&state).await {
        Some(s) => Ok(ServiceStatus {
            running: s.is_alive().await,
            port: Some(s.port),
            loaded: s.loaded_ids().await,
            models_dir,
        }),
        None => Ok(ServiceStatus {
            running: false,
            port: None,
            loaded: Vec::new(),
            models_dir,
        }),
    }
}

#[tauri::command]
pub async fn start_service(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<ServiceStatus, String> {
    ensure(&app, &state).await?;
    service_status(app, state).await
}

#[tauri::command]
pub async fn stop_service(app: AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.cancel.store(true, Ordering::Relaxed);
    let existing = {
        let mut guard = state.service.lock().await;
        guard.take()
    };
    if let Some(s) = existing {
        s.kill().await;
    }
    let _ = app.emit("service://state-changed", ());
    Ok(())
}

#[tauri::command]
pub async fn list_router_models(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<RouterModel>, String> {
    let s = ensure(&app, &state).await?;
    s.get_models().await
}

#[tauri::command]
pub async fn load_model(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    model: String,
) -> Result<(), String> {
    let s = ensure(&app, &state).await?;
    state.cancel.store(false, Ordering::Relaxed);
    s.load(&app, &model, &state.cancel).await?;
    *state.last_used.lock().await = Instant::now();
    Ok(())
}

#[tauri::command]
pub async fn unload_model(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    model: String,
) -> Result<(), String> {
    let Some(s) = current(&state).await else {
        return Ok(());
    };
    s.unload(&app, &model).await
}

/// 用户主动中止：停止正在生成的请求，并把正在加载的模型卸掉。
#[tauri::command]
pub async fn abort(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    model: Option<String>,
) -> Result<(), String> {
    state.cancel.store(true, Ordering::Relaxed);

    let Some(s) = current(&state).await else {
        return Ok(());
    };

    let targets = match model {
        Some(m) => vec![m],
        None => s.loaded_ids().await,
    };

    for m in targets {
        let state_now = s.state_of(&m).await.unwrap_or_default();
        if state_now == "loading" || state_now == "loaded" {
            let _ = s.unload(&app, &m).await;
        }
    }

    Ok(())
}

#[derive(Deserialize, Serialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    #[serde(default)]
    pub request_id: String,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    /// 本次对话的思考档位覆盖（default/off/low/medium/high/max）。
    /// 为空时用该模型在设置里保存的档位。
    #[serde(default)]
    pub thinking: Option<String>,
}

/// 请求把一段较早的对话压成摘要。哪一段算"较早"由前端决定。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactRequest {
    pub model: String,
    /// 待压缩的那批历史消息（已经是前端挑出来的旧片段）
    pub messages: Vec<ChatMessage>,
}

/// 通用的流式补全：负责加载模型、发请求、解析 SSE 并把 token 推给前端。
///
/// 取某个模型的调用画像。模型 id 就是 GGUF 文件名主干，据此反查路径。
///
/// **带进程内缓存**：这个函数在 `translate_segment` 里被逐段调用，
/// 而 `models::scan` 要递归读目录、逐个 stat。批量翻译 100 段就是 100 次扫描，
/// 白白拖慢任务。缓存按「模型 id + 目录」失效——换模型目录时会重新扫。
///
/// 查不到（比如模型刚被移走）时按通用模型处理——宁可走现有逻辑，
/// 也不要因为读不到元数据就让翻译直接失败。
fn model_profile(app: &AppHandle, model: &str) -> models::ModelProfile {
    fn cache() -> &'static std::sync::Mutex<HashMap<String, models::ModelProfile>> {
        static CACHE: std::sync::OnceLock<
            std::sync::Mutex<HashMap<String, models::ModelProfile>>,
        > = std::sync::OnceLock::new();
        CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
    }

    // 模型目录也算进键里：用户换目录后同名模型可能是完全不同的文件
    let dir = paths::models_dir(app)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let key = format!("{dir}\u{1}{model}");

    if let Some(hit) = cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }

    let profile = models::scan(app)
        .into_iter()
        .find(|m| m.id == model)
        .map(|m| models::model_caps(std::path::Path::new(&m.path)).profile)
        .unwrap_or_default();

    if let Ok(mut guard) = cache().lock() {
        // 键里带目录，旧条目留着也只是占点内存，简单限一下
        if guard.len() > 64 {
            guard.clear();
        }
        guard.insert(key, profile.clone());
    }

    profile
}

/// 同一个模型的机制只在首次使用时打印一次，避免批量翻译刷屏。
fn log_profile_once(model: &str, profile: &models::ModelProfile) {
    fn logged() -> &'static std::sync::Mutex<HashSet<String>> {
        static LOGGED: std::sync::OnceLock<std::sync::Mutex<HashSet<String>>> =
            std::sync::OnceLock::new();
        LOGGED.get_or_init(|| std::sync::Mutex::new(HashSet::new()))
    }

    let key = format!("{}\u{1}{}", model, profile.style.as_str());
    let first = logged()
        .lock()
        .map(|mut s| s.insert(key))
        .unwrap_or(false);
    if first {
        eprintln!(
            "翻译 [{}] 家族={} 机制={} 可对话={}",
            model,
            profile.family,
            profile.style.as_str(),
            profile.chat_capable
        );
    }
}

/// `thinking` 决定是否让思考型模型先输出思维链。翻译固定关闭：
/// 界面不展示思维链，它只会吃光 `max_tokens` 并拖慢首字。
async fn stream_completion(
    app: &AppHandle,
    s: &Service,
    request_id: &str,
    model: &str,
    messages: serde_json::Value,
    temperature: f64,
    max_tokens: u32,
    thinking: Thinking,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String, String> {
    stream_completion_with(app, s, request_id, model, messages, temperature, max_tokens, thinking, None, cancel).await
}

/// 同 [`stream_completion`]，但允许附加 `chat_template_kwargs`。
///
/// 有些翻译模型（translategemma）的语言方向完全由模板参数决定，
/// 不传就默认 en-GB，把任何输入都翻成英式英语。这类模型的参数
/// 必须和思考参数合并进同一个对象——llama.cpp 只认一个 `chat_template_kwargs`。
#[allow(clippy::too_many_arguments)]
async fn stream_completion_with(
    app: &AppHandle,
    s: &Service,
    request_id: &str,
    model: &str,
    messages: serde_json::Value,
    temperature: f64,
    max_tokens: u32,
    thinking: Thinking,
    extra_template_kwargs: Option<serde_json::Value>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String, String> {
    s.load(app, model, cancel).await?;

    let started = Instant::now();

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "temperature": temperature,
        "max_tokens": max_tokens,
        // 没有它，流式响应拿不到 usage，"上下文用了多少"就只能靠猜
        "stream_options": { "include_usage": true }
    });

    // chat_template_kwargs 只有一个槽位：思考控制和模型专属参数必须合并，
    // 后写覆盖先写会把思考控制弄丢。
    let mut kwargs = serde_json::Map::new();

    // 思考控制：与 llama.cpp Web UI 的请求体保持一致
    //   - enable_thinking 走对话模板
    //   - thinking_budget_tokens 由服务端硬限制思考长度（实测生效）
    //   - reasoning_control 表示预算由客户端掌控
    if thinking != Thinking::Default {
        kwargs.insert(
            "enable_thinking".into(),
            serde_json::json!(thinking.enabled()),
        );
        body["reasoning_control"] = serde_json::json!(true);

        if let Some(budget) = thinking.budget() {
            body["thinking_budget_tokens"] = serde_json::json!(budget);
        }
    }

    // 模型专属参数（如 translategemma 的 source/target_lang_code）
    if let Some(extra) = extra_template_kwargs {
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                kwargs.insert(k.clone(), v.clone());
            }
        }
    }

    if !kwargs.is_empty() {
        body["chat_template_kwargs"] = serde_json::Value::Object(kwargs);
    }

    let resp = s
        .client
        .post(s.url("/v1/chat/completions"))
        .bearer_auth(&s.key)
        .json(&body)
        .timeout(Duration::from_secs(300))
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("推理失败，HTTP {}", resp.status()));
    }

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut full = String::new();
    let mut finish_reason = String::new();
    let mut reasoning_chars = 0usize;
    let mut usage: Option<(u64, u64, u64)> = None;

    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let chunk = chunk.map_err(|e| format!("读取流失败: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buf.find('\n') {
            let line: String = buf.drain(..=pos).collect();
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                let choice = &value["choices"][0];

                // 思考型模型的思维链：内容照常推给对话页显示（用户需要知道模型还在思考），
                // 同时记下长度用于"空正文"的诊断
                if let Some(reasoning) = choice["delta"]["reasoning_content"].as_str() {
                    if !reasoning.is_empty() {
                        reasoning_chars += reasoning.chars().count();
                        if !request_id.is_empty() {
                            let _ = app.emit(
                                "llm://reasoning",
                                TokenPayload {
                                    request_id: request_id.to_string(),
                                    token: reasoning.to_string(),
                                },
                            );
                        }
                    }
                }
                if let Some(reason) = choice["finish_reason"].as_str() {
                    finish_reason = reason.to_string();
                }

                if let Some(token) = choice["delta"]["content"].as_str() {
                    if !token.is_empty() {
                        full.push_str(token);
                        // 空 request_id 表示"不需要推流"（批量任务），省掉无谓的事件
                        if !request_id.is_empty() {
                            let _ = app.emit(
                                "llm://token",
                                TokenPayload {
                                    request_id: request_id.to_string(),
                                    token: token.to_string(),
                                },
                            );
                        }
                    }
                }

                // 只有 stream_options.include_usage 打开时，最后一块才带 usage
                if let Some(u) = value["usage"].as_object() {
                    usage = Some((
                        u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        u.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                        u.get("prompt_tokens_details")
                            .and_then(|d| d.get("cached_tokens"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                    ));
                }
            }
        }
    }

    // 上报真实用量，让界面能显示"上下文用了多少"。
    // 放在空回复判断之前：预算被思考吃光时，正是最需要看到用量的场景。
    if !request_id.is_empty() {
        if let Some((prompt_tokens, completion_tokens, cached_tokens)) = usage {
            let _ = app.emit(
                "llm://usage",
                UsagePayload {
                    request_id: request_id.to_string(),
                    prompt_tokens,
                    completion_tokens,
                    cached_tokens,
                    ctx_size: params::ctx_of(app, model),
                    elapsed_ms: started.elapsed().as_millis() as u64,
                },
            );
        }
    }

    // 空正文绝不能当成功返回：上层会把它渲染成一个空气泡，用户以为"回复丢了"。
    if full.trim().is_empty() {
        if cancel.load(Ordering::Relaxed) {
            return Err("已取消".into());
        }
        return Err(empty_reply_message(
            reasoning_chars,
            max_tokens,
            &finish_reason,
        ));
    }

    // 有正文但被上限截断：至少留痕，方便排查长回答被切断
    if finish_reason == "length" {
        eprintln!("输出达到 max_tokens={max_tokens} 上限，可能被截断（模型 {model}）");
    }

    Ok(full)
}

/// 空回复的诊断文案。
///
/// 三种情况必须分开说，否则会给出误导性的建议：
/// - `length`：思考真的把输出额度吃光了——这才是"调低思考档位"的场景；
/// - 其它 finish_reason（或压根没收到）：**流被中断**，多半是模型进程在
///   生成期间没了（被卸载、崩溃、连接断开）。此时让用户"调低思考档位"
///   纯属误导，他们会调低后发现问题依旧。
/// - 有思考但没有任何正文且没收到 finish_reason：同上，按中断处理。
fn empty_reply_message(reasoning_chars: usize, max_tokens: u32, finish_reason: &str) -> String {
    let exhausted = finish_reason == "length";

    if exhausted {
        if reasoning_chars > 0 {
            return format!(
                "模型只输出了思考内容（{reasoning_chars} 字）就用完了 {max_tokens} token 的输出预算，正文为空。请把思考档位调低，或在设置里给这个模型更大的上下文。"
            );
        }
        return format!(
            "模型在 {max_tokens} token 的输出预算内没有产出正文。请把思考档位调低，或在设置里给这个模型更大的上下文。"
        );
    }

    // 没到上限却没有正文 → 生成被打断了，和预算无关
    if reasoning_chars > 0 {
        format!(
            "生成在思考阶段中断了（已输出 {reasoning_chars} 字思考内容），没有收到正文。多半是模型加载被中断或推理进程退出了，请重试。"
        )
    } else {
        "模型没有返回内容，请重试。".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_reply_blames_thinking_when_reasoning_present() {
        // 真正"思考吃光预算"才会带 finish_reason=length
        let message = empty_reply_message(2985, 1024, "length");
        // 实测 Qwen3.5 就是这个形态：思考 2985 字、正文 0 字
        assert!(message.contains("2985"), "应带上思考字数: {message}");
        assert!(message.contains("1024"), "应带上输出上限: {message}");
        assert!(message.contains("思考"), "应指出是思考吃掉了预算: {message}");
    }

    #[test]
    fn empty_reply_without_thinking_stays_generic() {
        assert_eq!(
            empty_reply_message(0, 1024, "length"),
            "模型在 1024 token 的输出预算内没有产出正文。请把思考档位调低，或在设置里给这个模型更大的上下文。"
        );
        assert_eq!(
            empty_reply_message(0, 1024, ""),
            "模型没有返回内容，请重试。"
        );
    }

    /// 流被中断时不能建议"调低思考档位"——那会导致用户按错误方向排查。
    #[test]
    fn interrupted_stream_is_not_reported_as_budget_exhaustion() {
        // 思考只输出 6 字就被打断：真实形态是 finish_reason 为空/stop
        let message = empty_reply_message(6, 1536, "");
        assert!(
            !message.contains("预算"),
            "中断不该说成预算耗尽: {message}"
        );
        assert!(!message.contains("调低"), "中断不该建议调低档位: {message}");
        assert!(message.contains("中断"), "应指出是中断: {message}");
        assert!(message.contains("6"), "应带上已输出的思考字数: {message}");
    }

    #[test]
    fn segment_messages_shapes_system_then_user() {
        // 消息形状现在由 prompts::build_request 统一产出，
        // 这里只保留一条最小断言，防止回归到"system+user"之外的结构。
        let req = prompts::build_request(
            models::PromptStyle::EnglishSystem,
            Style::General,
            "en",
            "zh",
            &[],
            "TEXT",
            None,
        );
        assert_eq!(req.messages[0]["role"], "system");
        assert_eq!(req.messages[1]["role"], "user");
        assert_eq!(req.messages[1]["content"], "TEXT");
    }

    /// 推理期间 `inflight` 必须大于 0，TTL 才不会误杀正在用的模型。
    #[test]
    fn inflight_guard_marks_busy_and_releases_on_drop() {
        let counter = Arc::new(AtomicU64::new(0));

        {
            let _guard = InflightGuard::new(&counter);
            assert_eq!(
                counter.load(Ordering::Relaxed),
                1,
                "推理开始时 TTL 就该让路"
            );
        }

        assert_eq!(
            counter.load(Ordering::Relaxed),
            0,
            "推理结束后必须归零，否则 TTL 会永久失效、显存再也不释放"
        );
    }

    /// 嵌套/并发守卫要能正确叠加，提前返回（`?`）时也必须回退。
    #[test]
    fn inflight_guard_nests_and_releases_on_early_return() {
        let counter = Arc::new(AtomicU64::new(0));

        fn bail(counter: &Arc<AtomicU64>) -> Result<(), String> {
            let _guard = InflightGuard::new(counter);
            Err("提前返回".into())
        }

        {
            let _outer = InflightGuard::new(&counter);
            assert_eq!(counter.load(Ordering::Relaxed), 1);
            assert!(bail(&counter).is_err());
            assert_eq!(
                counter.load(Ordering::Relaxed),
                1,
                "内层提前返回后应回退到只剩外层"
            );
        }

        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    fn stats() -> SegmentStats {
        SegmentStats {
            flagged: false,
            fixed: false,
        }
    }

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: content.to_string(),
        }
    }

    /// 预算是按 ctx 算的，不是写死的字符数。
    #[test]
    fn history_budget_scales_with_context() {
        assert!(history_budget(8192) > history_budget(4096));
        // 极小 ctx 也要留一点，不能算出 0
        assert!(history_budget(512) >= HISTORY_BUDGET_MIN);
    }

    /// 裁剪必须按轮次对齐：不能留下"没有提问的回答"。
    ///
    /// 用"一轮 = 短提问 + 长回答"来构造：如果按单条消息丢，
    /// 长回答会先撞上预算被丢掉、短提问却被留下，正好制造出一段
    /// 没有回答的孤儿提问；按轮次丢则整轮一起进出。
    #[test]
    fn trim_history_keeps_turns_aligned() {
        let short = "问".repeat(100);
        let long = "答".repeat(2000);

        let messages = vec![
            msg("user", &short),
            msg("assistant", &long), // 第 1 轮：短问 + 长答
            msg("user", &short),
            msg("assistant", &long), // 第 2 轮
            msg("user", "最后一句"),
        ];

        // 预算只够装最后一轮的多半，装不下完整的第 2 轮
        let trimmed = trim_history(&messages, 1200);

        // 按轮次丢：第 2 轮整轮被丢弃，不会留下孤零零的 user
        assert_eq!(trimmed.last().unwrap().content, "最后一句");
        assert_eq!(
            trimmed.len(),
            1,
            "装不下整轮就该整轮丢掉，实际保留了：{:?}",
            trimmed.iter().map(|m| m.role.as_str()).collect::<Vec<_>>()
        );
    }

    /// 相邻消息成对进出：绝不以 assistant 开头。
    #[test]
    fn trim_history_never_starts_with_assistant() {
        let long = "字".repeat(800);
        let messages = vec![
            msg("user", &long),
            msg("assistant", &long),
            msg("user", &long),
            msg("assistant", &long),
        ];

        for budget_ctx in [700, 1000, 1500, 2000, 3000] {
            let trimmed = trim_history(&messages, budget_ctx);
            if let Some(first) = trimmed.first() {
                assert_eq!(
                    first.role, "user",
                    "ctx={budget_ctx} 时裁出了 assistant 开头的历史"
                );
            }
        }
    }

    /// 最后一条无论多长都要带上——用户刚说的话不能丢。
    #[test]
    fn trim_history_always_keeps_the_latest_message() {
        let huge = "字".repeat(50_000);
        let messages = vec![msg("user", &huge)];

        let trimmed = trim_history(&messages, 4096);
        assert_eq!(trimmed.len(), 1);
        assert_eq!(trimmed[0].content.len(), huge.len());
    }

    #[test]
    fn trim_history_passes_short_conversations_through() {
        let messages = vec![
            msg("user", "你好"),
            msg("assistant", "你好！有什么可以帮你？"),
        ];
        assert_eq!(trim_history(&messages, 4096).len(), 2);
        assert!(trim_history(&[], 4096).is_empty());
    }

    /// 取消时必须拦住缓存写入：半截译文一旦进缓存就再也翻不掉了。
    #[test]
    fn cancelled_segment_is_never_cached() {
        let mut cache = HashMap::new();
        store_segment(&mut cache, "k".into(), "半截译".into(), stats(), true);
        assert!(cache.is_empty(), "被取消的片段不能写缓存");
    }

    #[test]
    fn finished_segment_is_cached_with_its_stats() {
        let mut cache = HashMap::new();
        let flagged = SegmentStats {
            flagged: true,
            fixed: true,
        };
        store_segment(&mut cache, "k".into(), "完整译文".into(), flagged, false);

        let (text, stored) = cache.get("k").expect("完整片段应该命中缓存");
        assert_eq!(text, "完整译文");
        // 统计要跟着一起存：批量重复跑时"检出多少段"才不会丢
        assert!(stored.flagged && stored.fixed);
    }

    #[test]
    fn cache_clears_itself_at_the_limit() {
        let mut cache = HashMap::new();
        for i in 0..CACHE_LIMIT {
            store_segment(&mut cache, format!("k{i}"), format!("v{i}"), stats(), false);
        }
        assert_eq!(cache.len(), CACHE_LIMIT);

        store_segment(&mut cache, "overflow".into(), "v".into(), stats(), false);
        // 超限就整体清空，只留下这一次写入
        assert_eq!(cache.len(), 1);
        assert!(cache.contains_key("overflow"));
    }
}

pub(crate) fn load_glossary(app: &AppHandle) -> Vec<GlossaryEntry> {
    paths::config_value(app, "glossary")
        .and_then(|v| serde_json::from_value::<Vec<GlossaryEntry>>(v).ok())
        .unwrap_or_default()
}

fn cache_key(
    style: Style,
    model: &str,
    source: &str,
    target: &str,
    glossary: &[GlossaryEntry],
    text: &str,
) -> String {
    let terms: Vec<String> = glossary
        .iter()
        .map(|e| format!("{}={}", e.from, e.to))
        .collect();
    format!(
        "{model}\u{1}{style}\u{1}{source}\u{1}{target}\u{1}{}\u{1}{text}",
        terms.join("\u{1}"),
        style = style.as_str()
    )
}

/// 长文本按段落/句子切分后逐段翻译，命中缓存时直接返回。
#[tauri::command]
pub async fn translate(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    request: TranslateRequest,
) -> Result<String, String> {
    if request.text.trim().is_empty() {
        return Err("请输入要翻译的内容".into());
    }

    let service = ensure(&app, &state).await?;
    let _infer = state.infer_lock.lock().await;
    // 拿到推理通道后立刻挂上守卫：从这一刻起 TTL 不再倒计时
    let _busy = InflightGuard::new(&state.inflight);
    state.cancel.store(false, Ordering::Relaxed);

    let glossary = load_glossary(&app);
    let style = Style::parse(&request.style);
    let segments = segmenter::split_segments(&request.text, 1200);
    let total = segments.len();
    let _ = app.emit("translate://progress", TranslateProgress { done: 0, total });

    let mut output = String::new();

    for (index, segment) in segments.iter().enumerate() {
        if state.cancel.load(Ordering::Relaxed) {
            break;
        }

        let (translated, _stats) = service
            .translate_segment(
                &app,
                &state,
                Some(&request.request_id),
                &request.model,
                &request.source,
                &request.target,
                style,
                &glossary,
                &segment.text,
                &state.cancel,
            )
            .await?;

        output.push_str(&translated);

        if index + 1 < total {
            let joiner = segmenter::joiner(segment.paragraph, &request.target);
            output.push_str(joiner);
            let _ = app.emit(
                "llm://token",
                TokenPayload {
                    request_id: request.request_id.clone(),
                    token: joiner.to_string(),
                },
            );
        }

        let _ = app.emit(
            "translate://progress",
            TranslateProgress {
                done: index + 1,
                total,
            },
        );
    }

    *state.last_used.lock().await = Instant::now();

    if state.cancel.load(Ordering::Relaxed) && output.is_empty() {
        return Err("已取消".into());
    }

    Ok(output)
}

#[tauri::command]
pub fn get_glossary(app: AppHandle) -> Vec<GlossaryEntry> {
    load_glossary(&app)
}

#[tauri::command]
pub fn set_glossary(
    app: AppHandle,
    entries: Vec<GlossaryEntry>,
) -> Result<(), String> {
    paths::set_config_value(&app, "glossary", serde_json::json!(entries))
}

/// 对话历史的内容预算（字符）。
///
/// 按默认 ctx 4096 算：输出预算最多 2048，留给输入约 2048 token ≈ 4000 字符。
/// 之前写 6000 是按 8192 ctx 拍的，对 4096 的模型其实已经超了；
/// 现在按"当前模型的 ctx"动态算，见 [`history_budget`]。
const HISTORY_BUDGET_RATIO: f64 = 0.45;

/// 至少给对话历史留这么多字符，免得极小 ctx 下一点上下文都带不上。
const HISTORY_BUDGET_MIN: usize = 1200;

/// 按模型上下文算出给"历史消息"用的字符预算。
///
/// 中英混排下大致 1 token ≈ 2 个中文字符（英文更省），这里取偏保守的换算，
/// 并且只把 ctx 的一小部分分给历史——剩下的要留给输出和本轮提问。
fn history_budget(ctx: u64) -> usize {
    let tokens = (ctx as f64 * HISTORY_BUDGET_RATIO) as usize;
    // 1 token ≈ 2 字符（中文为主时的经验值）
    (tokens * 2).max(HISTORY_BUDGET_MIN)
}

/// 摘要提示词：把"较早的一段译文修改对话"压缩成一份可接续的要点。
///
/// 刻意**不是**编程 agent 那套（Goal/Progress/Blocked/文件路径）——
/// 那是给改代码用的。这里是"改译文"，真正需要带下去的是：译的是什么、
/// 定了哪些译法、试过哪些方向、当前版本长什么样。
const COMPACT_PROMPT: &str = "The messages above are an earlier part of a translation revision session. \
Write a compact handover note so the work can continue without re-reading them.

Use EXACTLY this format (skip a section only if it truly does not apply):

## 翻译任务
- 原文要点 / 语种方向 / 采用的风格

## 术语与约束
- 用户明确要求过的措辞、术语、语气、禁忌

## 已确定的译法
- 已经敲定、后续必须延续的具体表达

## 修改历史
- 试过哪些方向，为什么放弃；哪一版被采纳

## 当前版本
- 最新的译文（若较长可只留关键段落）

Keep it under 400 words. Preserve exact wording the user approved, \
and never invent content that was not in the conversation.";

/// 把一段较早的对话压成摘要。
///
/// 只负责"摘要"这一件事：哪一段该压、压完怎么替换，由前端决定——
/// 前端才清楚哪些消息属于"旧"、哪些要原样保留。
#[tauri::command]
pub async fn compact_history(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    request: CompactRequest,
) -> Result<String, String> {
    if request.messages.is_empty() {
        return Err("没有需要压缩的内容".into());
    }

    let service = ensure(&app, &state).await?;
    let _infer = state.infer_lock.lock().await;
    // 摘要也是一次推理，同样不能让 TTL 把模型当成空闲
    let _busy = InflightGuard::new(&state.inflight);
    state.cancel.store(false, Ordering::Relaxed);

    // 摘要请求本身不该被再次压缩，直接按角色拼成一段文本喂进去，
    // 避免模型把待总结的内容当成"要继续的对话"。
    let transcript: String = request
        .messages
        .iter()
        .map(|m| {
            let who = match m.role.as_str() {
                "user" => "用户",
                "assistant" => "助手",
                other => other,
            };
            format!("[{who}]\n{}\n", m.content)
        })
        .collect();

    let messages = serde_json::json!([
        { "role": "system", "content": COMPACT_PROMPT },
        { "role": "user", "content": transcript }
    ]);

    let model_params = params::get(&app, &request.model);
    // 摘要要短，给 1024 足够；思考关闭，否则小模型会把额度全花在思考上
    let budget = 1024u32.min((model_params.ctx / 4).max(256) as u32);

    let summary = stream_completion(
        &app,
        &service,
        "", // 空 request_id：不向前端推 token
        &request.model,
        messages,
        0.1,
        budget,
        Thinking::Off,
        &state.cancel,
    )
    .await;

    *state.last_used.lock().await = Instant::now();

    match summary {
        Ok(text) => Ok(text),
        Err(_) if state.cancel.load(Ordering::Relaxed) => Err("已取消".into()),
        Err(e) => Err(e),
    }
}
/// 裁剪对话历史，使其落在预算内。
///
/// 两条规则，都来自"对话是成对出现的"这个事实：
/// 1. **按轮次丢**：一轮 = 一条用户提问 + 它后面的一条助手回复。只按字符丢会把
///    一轮砍成半截（比如留下 assistant 却丢掉它回答的那条 user），模型看到一段
///    没有提问的回答会莫名其妙。
/// 2. **保留最后一条**：哪怕它自己就超预算，也要带上——用户刚说的话不能丢。
///
/// 与旧实现的差别：旧版从后往前累加字符、遇到超限就 `break`，同样会切出
/// "assistant 开头"的残缺历史。
fn trim_history(messages: &[ChatMessage], ctx: u64) -> Vec<ChatMessage> {
    if messages.is_empty() {
        return Vec::new();
    }

    let budget = history_budget(ctx);

    // 先按轮次分组：以 user 为界，把紧随其后的 assistant 归进同一轮。
    // 开头若有不属于任何 user 的消息（历史遗留的 system / 孤立的 assistant），
    // 单独成一组，这类通常优先级最低。
    let mut turns: Vec<Vec<ChatMessage>> = Vec::new();
    for message in messages {
        let starts_turn = message.role == "user" || turns.is_empty();
        if starts_turn {
            turns.push(vec![message.clone()]);
        } else if let Some(last) = turns.last_mut() {
            last.push(message.clone());
        }
    }

    // 从最新的轮次往回装，装不下就停。最后一条无论如何都要保留。
    let mut kept: Vec<Vec<ChatMessage>> = Vec::new();
    let mut used = 0usize;

    for (index, turn) in turns.iter().enumerate().rev() {
        let size: usize = turn.iter().map(|m| m.content.chars().count()).sum();
        let is_last = index == turns.len() - 1;

        if !is_last && used + size > budget {
            break;
        }
        used += size;
        kept.push(turn.clone());

        // 已经装超了就别再往前加（最后一条超预算的情况）
        if used > budget {
            break;
        }
    }

    kept.reverse();
    kept.into_iter().flatten().collect()
}

#[tauri::command]
pub async fn chat(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    request: ChatRequest,
) -> Result<String, String> {
    if request.messages.is_empty() {
        return Err("消息为空".into());
    }

    let s = ensure(&app, &state).await?;
    let _infer = state.infer_lock.lock().await;
    // 思考型模型可能一跑好几分钟，这段时间绝不能算作"空闲"
    let _busy = InflightGuard::new(&state.inflight);
    state.cancel.store(false, Ordering::Relaxed);

    // 对话允许按模型（或按这次会话）选择思考档位
    let model_params = params::get(&app, &request.model);

    // 历史预算跟着该模型的 ctx 走：同一段对话换到小 ctx 模型时不能还按大预算裁
    let trimmed = trim_history(&request.messages, model_params.ctx);
    let messages = serde_json::to_value(&trimmed).map_err(|e| e.to_string())?;
    let thinking = request
        .thinking
        .as_deref()
        .map(Thinking::parse)
        .unwrap_or(model_params.thinking);

    // 思考与正文共用同一份输出额度，额度按档位算（含"不限"档的特殊处理）。
    // 规则与界面提示"上下文不足"用的是同一个函数，避免两边漂移。
    let max_tokens = thinking.output_budget(model_params.ctx);

    // 对话页**不重建消息**：这里的 messages 就是用户的对话本身，
    // 塞进翻译提示词会污染上下文。真正需要适配的只有模板参数——
    // translategemma 如果被强行用在对话页（比如改了文件名绕过过滤），
    // 不传语言码它会默认 en-GB。正常流程下它已被过滤，这里只是兜底。
    let profile = model_profile(&app, &request.model);
    log_profile_once(&request.model, &profile);
    let template_kwargs = if profile.style == models::PromptStyle::TemplateKwargs {
        Some(serde_json::json!({
            "source_lang_code": "en-GB",
            "target_lang_code": "zh-Hans",
            "enable_thinking": thinking.enabled()
        }))
    } else {
        None
    };

    let result = stream_completion_with(
        &app,
        &s,
        &request.request_id,
        &request.model,
        messages,
        0.7,
        max_tokens,
        thinking,
        template_kwargs,
        &state.cancel,
    )
    .await;

    *state.last_used.lock().await = Instant::now();

    match result {
        Ok(text) => Ok(text),
        Err(_) if state.cancel.load(Ordering::Relaxed) => Err("已取消".into()),
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub async fn set_ttl(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    seconds: u64,
) -> Result<(), String> {
    state.ttl_secs.store(seconds, Ordering::Relaxed);
    paths::set_config_value(&app, "ttlSecs", serde_json::json!(seconds))?;
    *state.last_used.lock().await = Instant::now();
    Ok(())
}

#[tauri::command]
pub fn set_gpu_layers(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    layers: u64,
) -> Result<(), String> {
    state.gpu_layers.store(layers, Ordering::Relaxed);
    paths::set_config_value(&app, "gpuLayers", serde_json::json!(layers))?;
    Ok(())
}

#[tauri::command]
pub async fn set_pinned(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    model: String,
    pinned: bool,
) -> Result<Vec<String>, String> {
    let list = {
        let mut set = state.pinned.lock().await;
        if pinned {
            set.insert(model);
        } else {
            set.remove(&model);
        }
        let mut list: Vec<String> = set.iter().cloned().collect();
        list.sort();
        list
    };

    paths::set_config_value(&app, "pinned", serde_json::json!(list))?;
    *state.last_used.lock().await = Instant::now();
    Ok(list)
}

#[tauri::command]
pub async fn get_settings(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Settings, String> {
    let (running, port) = match current(&state).await {
        Some(s) => (s.is_alive().await, Some(s.port)),
        None => (false, None),
    };

    let pinned: Vec<String> = {
        let set = state.pinned.lock().await;
        let mut list: Vec<String> = set.iter().cloned().collect();
        list.sort();
        list
    };

    Ok(Settings {
        ttl_secs: state.ttl_secs.load(Ordering::Relaxed),
        gpu_layers: state.gpu_layers.load(Ordering::Relaxed),
        pinned,
        models_dir: paths::models_dir(&app)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        running,
        port,
    })
}

/// 思考档位的说明数据，供界面显示"最多 X token"。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingTier {
    pub value: String,
    /// 该档的思考预算；`-1` 表示不限，`None` 表示不设置
    pub budget_tokens: Option<i64>,
}

/// 列出所有思考档位。前端据此渲染，不在前端另抄一份数字。
#[tauri::command]
pub fn thinking_tiers() -> Vec<ThinkingTier> {
    const ALL: [Thinking; 6] = [
        Thinking::Default,
        Thinking::Off,
        Thinking::Low,
        Thinking::Medium,
        Thinking::High,
        Thinking::Max,
    ];

    ALL.iter()
        .map(|t| ThinkingTier {
            value: t.as_str().to_string(),
            budget_tokens: t.budget(),
        })
        .collect()
}

/// 模型目录里的一个文件，附带当前生效的参数，供设置抽屉直接渲染。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelInfo {
    pub id: String,
    pub path: String,
    pub size_mb: u64,
    /// 当前生效的上下文。改完需要重启引擎才生效。
    pub ctx_size: u64,
    /// 模型原生上下文上限（读 GGUF 元数据）；读不到时为 None。
    pub trained_ctx: Option<u64>,
    pub thinking: String,
    /// 对话模板是否支持思考开关（读 GGUF 里的 chat_template 判断）。
    /// 为 false 时界面应禁用思考档位，而不是让用户白设。
    pub supports_thinking: bool,
    /// 能否在对话页使用。翻译专用模型为 false——它们会把问题本身翻译掉，
    /// 或者编造身份，放进对话页只会让用户以为工具坏了。
    pub chat_capable: bool,
    /// 家族名，用于界面徽章（"Hy-MT2"、"TranslateGemma"…）。
    pub family: String,
    /// 完全用不了时的原因；为空表示可用。
    pub unsupported_reason: Option<String>,
    /// 自动识别的结果（不受用户覆盖影响），供界面显示"自动判定为…"。
    pub chat_auto: bool,
    /// 用户是否手动改过"能不能对话"。
    pub chat_overridden: bool,
}

#[tauri::command]
pub fn list_local_models(app: AppHandle) -> Vec<LocalModelInfo> {
    models::scan(&app)
        .into_iter()
        .map(|m| {
            let p = params::get(&app, &m.id);
            let path = std::path::Path::new(&m.path);
            let caps = models::model_caps(path);

            LocalModelInfo {
                trained_ctx: caps.trained_ctx,
                supports_thinking: caps.supports_thinking,
                ctx_size: p.ctx,
                thinking: p.thinking.as_str().to_string(),
                // 用户的覆盖优先于自动识别：识别靠读 GGUF 特征，改了文件名
                // 或遇到冷门模型时会认错，得让人能改回来。
                chat_capable: p.chat_override.unwrap_or(caps.profile.chat_capable),
                chat_auto: caps.profile.chat_capable,
                chat_overridden: p.chat_override.is_some(),
                family: caps.profile.family,
                unsupported_reason: caps.profile.unsupported,
                id: m.id,
                path: m.path,
                size_mb: m.size_mb,
            }
        })
        .collect()
}

/// 保存单个模型的上下文与思考开关。上下文改动需重启引擎生效。
#[tauri::command]
pub fn set_model_params(
    app: AppHandle,
    model: String,
    ctx: u64,
    thinking: String,
) -> Result<params::ModelParams, String> {
    // 保留已有的能力覆盖：这个命令只负责上下文与思考档位，
    // 若在这里重建 ModelParams 会把 chat_override 抹掉。
    let existing = params::get(&app, &model);
    params::set(
        &app,
        &model,
        &params::ModelParams {
            ctx,
            thinking: Thinking::parse(&thinking),
            chat_override: existing.chat_override,
        },
    )
}

/// 手动指定某个模型"能不能用在对话页"。传 `null` 恢复自动识别。
#[tauri::command]
pub fn set_model_chat_capable(
    app: AppHandle,
    model: String,
    capable: Option<bool>,
) -> Result<(), String> {
    params::set_chat_override(&app, &model, capable)
}

/// 改完 GPU 档位后重启引擎，让新参数生效。
#[tauri::command]
pub async fn restart_service(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<ServiceStatus, String> {
    {
        let existing = {
            let mut guard = state.service.lock().await;
            guard.take()
        };
        if let Some(s) = existing {
            s.kill().await;
        }
    }
    ensure(&app, &state).await?;
    service_status(app, state).await
}

/// 空闲 TTL 守护：停手若干秒后自动卸载，把显存还回去。
///
/// 注意这里是「用户停手」的计时，不是「距上次刷新多久」的计时：
/// 只要还有推理在跑（`inflight > 0`），就说明用户正用着，模型绝不能卸。
/// 否则思考型模型跑到一半被卸载，界面会只留下一小段内容并报出
/// 一个自相矛盾的「思考 N 字就用完预算」错误。
pub fn spawn_ttl_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;

            let state = app.state::<AppState>();
            let ttl = state.ttl_secs.load(Ordering::Relaxed);
            if ttl == 0 {
                continue;
            }

            // 正在推理 / 排队中：用户在用它，不算空闲
            if state.inflight.load(Ordering::Relaxed) > 0 {
                continue;
            }

            let idle = state.last_used.lock().await.elapsed();
            if idle < Duration::from_secs(ttl) {
                continue;
            }

            let Some(s) = current(&state).await else {
                continue;
            };
            let loaded = s.loaded_ids().await;
            if loaded.is_empty() {
                continue;
            }

            // 常驻模型永不自动卸载
            let pinned = state.pinned.lock().await.clone();
            let pending: Vec<String> = loaded
                .into_iter()
                .filter(|id| !pinned.contains(id))
                .collect();
            if pending.is_empty() {
                continue;
            }

            for id in pending {
                let _ = s.unload(&app, &id).await;
            }
        }
    });
}
