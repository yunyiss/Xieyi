use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// 应用数据目录（Windows: %APPDATA%\<identifier>）。
pub fn app_data(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

/// 运行时根目录：`<app_data>/runtime`。
pub fn runtime_root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data(app)?.join("runtime"))
}

/// 运行时清单文件。
pub fn runtime_json(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(runtime_root(app)?.join("runtime.json"))
}

/// 配置文件（用户设置）。
pub fn config_json(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data(app)?.join("config.json"))
}

/// 模型目录：优先取用户配置，否则用文档目录下的默认位置。
pub fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(dir) = std::fs::read_to_string(config_json(app)?)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("modelsDir").and_then(|d| d.as_str()).map(String::from))
    {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }

    app.path()
        .document_dir()
        .map(|d| d.join("LocalTranslate").join("models"))
        .map_err(|e| e.to_string())
}

/// 记录用户指定的模型目录。
pub fn set_models_dir(app: &AppHandle, dir: &str) -> Result<(), String> {
    let path = config_json(app)?;
    let mut value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    value["modelsDir"] = serde_json::Value::String(dir.to_string());
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap())
        .map_err(|e| format!("写入配置失败: {e}"))
}

/// 确保目录存在。
pub fn ensure(dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败 {}: {e}", dir.display()))
}
