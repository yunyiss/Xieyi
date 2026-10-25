use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use crate::models;
use crate::paths;
use crate::runtime;

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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ModelStatePayload {
    model: String,
    state: String,
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
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            service: tokio::sync::Mutex::new(None),
            infer_lock: tokio::sync::Mutex::new(()),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_used: tokio::sync::Mutex::new(Instant::now()),
            ttl_secs: AtomicU64::new(120),
            gpu_layers: AtomicU64::new(999),
            pinned: tokio::sync::Mutex::new(HashSet::new()),
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
    async fn wait_loaded(
        &self,
        model: &str,
        timeout: Duration,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<(), String> {
        let started = Instant::now();
        let mut seen_loading = false;

        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("已取消".into());
            }

            let state = self.state_of(model).await?;
            match state.as_str() {
                "loaded" => return Ok(()),
                "loading" => seen_loading = true,
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

        let _ = app.emit(
            "model://state-changed",
            ModelStatePayload {
                model: model.to_string(),
                state: "loading".into(),
            },
        );
        self.post_json("/models/load", serde_json::json!({ "model": model }))
            .await?;

        let outcome = self
            .wait_loaded(model, Duration::from_secs(180), cancel)
            .await;

        if outcome.is_err() {
            // 取消或超时：把半加载的实例卸掉，避免显存被占住
            let _ = self
                .post_json("/models/unload", serde_json::json!({ "model": model }))
                .await;
            self.loaded.lock().await.retain(|m| m != model);
            let _ = app.emit(
                "model://state-changed",
                ModelStatePayload {
                    model: model.to_string(),
                    state: "cold".into(),
                },
            );
            return outcome;
        }

        let mut loaded = self.loaded.lock().await;
        if !loaded.iter().any(|m| m == model) {
            loaded.push(model.to_string());
        }
        drop(loaded);

        let _ = app.emit(
            "model://state-changed",
            ModelStatePayload {
                model: model.to_string(),
                state: "warm".into(),
            },
        );
        Ok(())
    }

    pub async fn unload(&self, app: &AppHandle, model: &str) -> Result<(), String> {
        let _ = app.emit(
            "model://state-changed",
            ModelStatePayload {
                model: model.to_string(),
                state: "unloading".into(),
            },
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
            ModelStatePayload {
                model: model.to_string(),
                state: "cold".into(),
            },
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
    let preset = models::write_preset(app, &files, gpu_layers as u32)?;

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
}

/// 通用的流式补全：负责加载模型、发请求、解析 SSE 并把 token 推给前端。
async fn stream_completion(
    app: &AppHandle,
    s: &Service,
    request_id: &str,
    model: &str,
    messages: serde_json::Value,
    temperature: f64,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String, String> {
    s.load(app, model, cancel).await?;

    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "temperature": temperature,
        "max_tokens": 2048
    });

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
                if let Some(token) = value["choices"][0]["delta"]["content"].as_str() {
                    if !token.is_empty() {
                        full.push_str(token);
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
        }
    }

    Ok(full)
}

fn system_prompt(source: &str, target: &str) -> String {
    let src = if source == "auto" { "the source language" } else { source };
    format!(
        "You are a professional translator. Translate the user's text from {src} to {target}. \
Output only the translation, without explanations, notes, or quotation marks."
    )
}

#[tauri::command]
pub async fn translate(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    request: TranslateRequest,
) -> Result<String, String> {
    if request.text.trim().is_empty() {
        return Err("请输入要翻译的内容".into());
    }

    let s = ensure(&app, &state).await?;
    let _infer = state.infer_lock.lock().await;
    state.cancel.store(false, Ordering::Relaxed);

    let messages = serde_json::json!([
        { "role": "system", "content": system_prompt(&request.source, &request.target) },
        { "role": "user", "content": request.text }
    ]);

    let result = stream_completion(
        &app,
        &s,
        &request.request_id,
        &request.model,
        messages,
        0.2,
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

    let messages = serde_json::to_value(&request.messages).map_err(|e| e.to_string())?;

    let result = stream_completion(
        &app,
        &s,
        &request.request_id,
        &request.model,
        messages,
        0.7,
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

#[tauri::command]
pub fn list_local_models(app: AppHandle) -> Vec<models::ModelFile> {
    models::scan(&app)
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
