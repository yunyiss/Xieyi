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

/// 读取运行环境信息。
/// 目前只做两件事：报告平台信息、检查运行时是否已就位。
/// 后续的下载 / 自检逻辑会在此基础上扩展。
#[tauri::command]
fn app_info(app: tauri::AppHandle) -> Result<AppInfo, String> {
    let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let models_dir = app
        .path()
        .document_dir()
        .map(|d| d.join("LocalTranslate").join("models"))
        .map_err(|e| e.to_string())?;

    let runtime_version = std::fs::read_to_string(data_dir.join("runtime").join("runtime.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| {
            v.get("version")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        });

    Ok(AppInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: data_dir.to_string_lossy().to_string(),
        models_dir: models_dir.to_string_lossy().to_string(),
        runtime_version,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![app_info])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
