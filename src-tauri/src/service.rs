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
    pub text: String,
}

pub struct AppState {
    pub service: tokio::sync::Mutex<Option<Arc<Service>>>,
    /// 串行化推理请求，避免同时向单模型服务打并发；不阻塞状态查询。
    pub infer_lock: tokio::sync::Mutex<()>,
    /// 当前请求是否被用户取消。
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    pub last_used: tokio::sync::Mutex<Instant>,
    pub ttl_secs: AtomicU64,
    pub gpu_layers: AtomicU64,
    /// 常驻（Pinned）模型：永不自动卸载。
    pub pinned: tokio::sync::Mutex<HashSet<String>>,
    /// 句子级翻译缓存：相同模型 / 语言对 / 术语表 / 原文直接命中。
    pub cache: tokio::sync::Mutex<HashMap<String, String>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            service: tokio::sync::Mutex::new(None),
            infer_lock: tokio::sync::Mutex::new(()),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_used: tokio::sync::Mutex::new(Instant::now()),
            ttl_secs: AtomicU64::new(120),
            gpu_layers: AtomicU64::new(models::AUTO_LAYERS),
            pinned: tokio::sync::Mutex::new(HashSet::new()),
            cache: tokio::sync::Mutex::new(HashMap::new()),
        }
    }
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

    /// 翻译单个片段：命中缓存直接返回，否则流式推理并回填缓存。
    ///
    /// `request_id` 为 `None` 时不会向前端推送 token —— 批量任务走这条路径，
    /// 避免把批量过程的中间结果灌进翻译页的输出框。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn translate_segment(
        &self,
        app: &AppHandle,
        state: &AppState,
        request_id: Option<&str>,
        model: &str,
        source: &str,
        target: &str,
        glossary: &[GlossaryEntry],
        text: &str,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<String, String> {
        let key = cache_key(model, source, target, glossary, text);

        if let Some(hit) = state.cache.lock().await.get(&key).cloned() {
            if let Some(id) = request_id {
                let _ = app.emit(
                    "llm://token",
                    TokenPayload {
                        request_id: id.to_string(),
                        token: hit.clone(),
                    },
                );
            }
            return Ok(hit);
        }

        let messages = serde_json::json!([
            { "role": "system", "content": system_prompt(source, target, glossary) },
            { "role": "user", "content": text }
        ]);

        let translated = stream_completion(
            app,
            self,
            request_id.unwrap_or_default(),
            model,
            messages,
            0.2,
            1024,
            // 翻译不让模型思考：既慢又容易把输出预算吃光
            Thinking::Off,
            cancel,
        )
        .await?;

        let mut cache = state.cache.lock().await;
        if cache.len() > 4000 {
            cache.clear();
        }
        cache.insert(key, translated.clone());
        Ok(translated)
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

/// 通用的流式补全：负责加载模型、发请求、解析 SSE 并把 token 推给前端。
///
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

    // 思考控制：与 llama.cpp Web UI 的请求体保持一致
    //   - enable_thinking 走对话模板
    //   - thinking_budget_tokens 由服务端硬限制思考长度（实测生效）
    //   - reasoning_control 表示预算由客户端掌控
    if thinking != Thinking::Default {
        body["chat_template_kwargs"] =
            serde_json::json!({ "enable_thinking": thinking.enabled() });
        body["reasoning_control"] = serde_json::json!(true);

        if let Some(budget) = thinking.budget() {
            body["thinking_budget_tokens"] = serde_json::json!(budget);
        }
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

                // 思考型模型的思维链：不展示，但要记下长度用于诊断
                if let Some(reasoning) = choice["delta"]["reasoning_content"].as_str() {
                    reasoning_chars += reasoning.chars().count();
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
        return Err(empty_reply_message(reasoning_chars, max_tokens));
    }

    // 有正文但被上限截断：至少留痕，方便排查长回答被切断
    if finish_reason == "length" {
        eprintln!("输出达到 max_tokens={max_tokens} 上限，可能被截断（模型 {model}）");
    }

    Ok(full)
}

/// 空回复的诊断文案。实测 Qwen3.5 会先思考再作答，
/// 思维链把 `max_tokens` 吃光时正文就是空的（`finish_reason = length`）。
fn empty_reply_message(reasoning_chars: usize, max_tokens: u32) -> String {
    if reasoning_chars > 0 {
        format!(
            "模型只输出了思考内容（{reasoning_chars} 字）就用完了 {max_tokens} token 的输出预算，正文为空。请把思考档位调低，或在设置里给这个模型更大的上下文。"
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
        let message = empty_reply_message(2985, 1024);
        // 实测 Qwen3.5 就是这个形态：思考 2985 字、正文 0 字
        assert!(message.contains("2985"), "应带上思考字数: {message}");
        assert!(message.contains("1024"), "应带上输出上限: {message}");
        assert!(message.contains("思考"), "应指出是思考吃掉了预算: {message}");
    }

    #[test]
    fn empty_reply_without_thinking_stays_generic() {
        assert_eq!(empty_reply_message(0, 1024), "模型没有返回内容，请重试。");
    }
}

pub(crate) fn load_glossary(app: &AppHandle) -> Vec<GlossaryEntry> {
    paths::config_value(app, "glossary")
        .and_then(|v| serde_json::from_value::<Vec<GlossaryEntry>>(v).ok())
        .unwrap_or_default()
}

fn system_prompt(source: &str, target: &str, glossary: &[GlossaryEntry]) -> String {
    let src = if source == "auto" { "the source language" } else { source };
    let mut prompt = format!(
        "You are a professional translator. Translate the user's text from {src} to {target}. \
Output only the translation, without explanations, notes, or quotation marks."
    );

    if !glossary.is_empty() {
        prompt.push_str("\nUse these term mappings when they appear:\n");
        for entry in glossary {
            prompt.push_str(&format!("- {} => {}\n", entry.from, entry.to));
        }
    }

    prompt
}

fn cache_key(
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
        "{model}\u{1}{source}\u{1}{target}\u{1}{}\u{1}{text}",
        terms.join("\u{1}")
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
    state.cancel.store(false, Ordering::Relaxed);

    let glossary = load_glossary(&app);
    let segments = segmenter::split_segments(&request.text, 1200);
    let total = segments.len();
    let _ = app.emit("translate://progress", TranslateProgress { done: 0, total });

    let mut output = String::new();

    for (index, segment) in segments.iter().enumerate() {
        if state.cancel.load(Ordering::Relaxed) {
            break;
        }

        let translated = service
            .translate_segment(
                &app,
                &state,
                Some(&request.request_id),
                &request.model,
                &request.source,
                &request.target,
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

/// 对话历史的字符预算（约 3000 tokens）。
/// 超出时从最早的消息开始丢弃，避免把上下文窗口撑爆。
const CHAT_HISTORY_CHARS: usize = 6000;

fn trim_history(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut total = 0usize;
    let mut keep: Vec<ChatMessage> = Vec::new();

    for message in messages.iter().rev() {
        let len = message.content.chars().count();
        if !keep.is_empty() && total + len > CHAT_HISTORY_CHARS {
            break;
        }
        total += len;
        keep.push(message.clone());
    }

    keep.reverse();
    keep
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
    state.cancel.store(false, Ordering::Relaxed);

    let trimmed = trim_history(&request.messages);
    let messages = serde_json::to_value(&trimmed).map_err(|e| e.to_string())?;

    // 对话允许按模型（或按这次会话）选择思考档位
    let model_params = params::get(&app, &request.model);
    let thinking = request
        .thinking
        .as_deref()
        .map(Thinking::parse)
        .unwrap_or(model_params.thinking);

    // 思考与正文共用同一份输出额度，所以要把思考的份额加进 max_tokens。
    // 再夹到 ctx 的一半，给 prompt 留出空间，避免一开思考就撑爆上下文。
    let max_tokens = (1024 + thinking.reserved_tokens())
        .min((model_params.ctx / 2).max(512) as u32);

    let result = stream_completion(
        &app,
        &s,
        &request.request_id,
        &request.model,
        messages,
        0.7,
        max_tokens,
        thinking,
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
    params::set(
        &app,
        &model,
        &params::ModelParams {
            ctx,
            thinking: Thinking::parse(&thinking),
        },
    )
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
pub fn spawn_ttl_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;

            let state = app.state::<AppState>();
            let ttl = state.ttl_secs.load(Ordering::Relaxed);
            if ttl == 0 {
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
