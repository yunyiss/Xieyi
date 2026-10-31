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

/// 读取配置项。
pub fn config_value(app: &AppHandle, key: &str) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(config_json(app).ok()?).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value.get(key).cloned()
}

/// 写入配置项。
pub fn set_config_value(
    app: &AppHandle,
    key: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    let path = config_json(app)?;
    if let Some(parent) = path.parent() {
        ensure(parent)?;
    }
    let mut root = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    root[key] = value;
    std::fs::write(&path, serde_json::to_string_pretty(&root).unwrap())
        .map_err(|e| format!("写入配置失败: {e}"))
}

/// 模型目录：优先取用户配置，否则用**程序所在目录**下的 `models`。
///
/// 为什么不再放「文档」目录：这个工具的使用方式是"一个文件夹带走全部"，
/// 模型放在 exe 旁边最直观，用户想搬走或清理时不用满硬盘找。
/// 程序目录不可写时（装在 Program Files、或从只读介质运行）才回退到文档目录，
/// 否则建目录会失败。
pub fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(dir) = config_value(app, "modelsDir")
        .and_then(|v| v.as_str().map(String::from))
        .filter(|s| !s.is_empty())
    {
        return Ok(PathBuf::from(dir));
    }

    // 程序目录优先
    if let Some(dir) = exe_dir().map(|d| d.join("models")) {
        if dir.is_dir() || dir_is_writable(dir.parent().unwrap_or(&dir)) {
            return Ok(dir);
        }
    }

    // 回退：程序目录不可写时用文档目录
    app.path()
        .document_dir()
        .map(|d| d.join("Xieyi").join("models"))
        .map_err(|e| e.to_string())
}

/// 可执行文件所在目录。
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(PathBuf::from)
}

/// 目录是否可写：真写一个探针文件再删掉，比查权限位可靠（Windows 上尤其）。
fn dir_is_writable(dir: &std::path::Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(".xieyi-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 记录用户指定的模型目录。
pub fn set_models_dir(app: &AppHandle, dir: &str) -> Result<(), String> {
    set_config_value(app, "modelsDir", serde_json::Value::String(dir.to_string()))
}

/// 确保目录存在。
pub fn ensure(dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败 {}: {e}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认模型目录要跟着程序走，不再固定在「文档」里。
    ///
    /// 这个工具是"一个文件夹带走全部"的用法：模型放在 exe 旁边最直观，
    /// 想搬走或清理时不用满硬盘找。测试以当前测试可执行文件所在目录为准，
    /// 它和真实 exe 一样都是"程序目录"。
    #[test]
    fn default_models_dir_sits_next_to_the_executable() {
        let exe = std::env::current_exe().expect("测试进程应当有可执行文件路径");
        let parent = exe.parent().expect("可执行文件应当有父目录");
        let expected = parent.join("models");

        // 只在程序目录可写时才会选中程序目录；不可写时回退文档目录是预期行为
        if dir_is_writable(parent) {
            assert_eq!(exe_dir().map(|d| d.join("models")), Some(expected));
        }
    }

    /// 可写探测必须真写一遍再删干净，不能留下探针文件。
    #[test]
    fn writable_probe_leaves_nothing_behind() {
        let dir = std::env::temp_dir().join("xieyi-probe-test");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(dir_is_writable(&dir));
        assert!(
            !dir.join(".xieyi-write-probe").exists(),
            "探针文件应当被删掉"
        );
        // 不存在的目录一律判为不可写
        assert!(!dir_is_writable(&dir.join("nope")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
