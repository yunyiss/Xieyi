mod hardware;
mod models;
mod paths;
mod runtime;
mod service;
mod translate;

use serde::Serialize;
use tauri::Manager;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    os: String,
    arch: String,
    version: String,
    data_dir: String,
    models_dir: String,
    runtime_version: Option<String>,
}

#[tauri::command]
fn app_info(app: tauri::AppHandle) -> Result<AppInfo, String> {
    let data_dir = paths::app_data(&app)?;
    let models_dir = paths::models_dir(&app)?;
    let status = runtime::status(&app);

    Ok(AppInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: data_dir.to_string_lossy().to_string(),
        models_dir: models_dir.to_string_lossy().to_string(),
        runtime_version: status.version,
    })
}

#[tauri::command]
fn open_models_dir(app: tauri::AppHandle) -> Result<String, String> {
    let dir = paths::models_dir(&app)?;
    paths::ensure(&dir)?;
    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
fn set_models_dir(app: tauri::AppHandle, path: String) -> Result<String, String> {
    let dir = std::path::PathBuf::from(&path);
    if !dir.is_dir() {
        return Err(format!("目录不存在: {path}"));
    }
    paths::set_models_dir(&app, &path)?;
    Ok(path)
}

#[tauri::command]
fn probe_vram() -> Option<u64> {
    hardware::free_vram_mb()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(service::AppState::default())
        .setup(|app| {
            service::load_settings(app.handle());
            service::spawn_ttl_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            open_models_dir,
            set_models_dir,
            probe_vram,
            runtime::runtime_status,
            runtime::install_runtime,
            runtime::set_manual_runtime,
            service::service_status,
            service::start_service,
            service::stop_service,
            service::list_router_models,
            service::load_model,
            service::unload_model,
            service::translate,
            service::get_glossary,
            service::set_glossary,
            service::chat,
            service::abort,
            service::set_ttl,
            service::set_gpu_layers,
            service::set_pinned,
            service::get_settings,
            service::list_local_models,
            service::restart_service,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // 退出时确保推理进程树被回收，避免僵尸进程占着显存。
                let state = app.state::<service::AppState>();
                tauri::async_runtime::block_on(async {
                    let mut guard = state.service.lock().await;
                    if let Some(s) = guard.take() {
                        s.kill().await;
                    }
                });
            }
        });
}
