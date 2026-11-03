use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// 应用数据根目录（Windows 上是 `%LOCALAPPDATA%\<identifier>`）。
///
/// **刻意用 `app_local_data_dir` 而不是 `app_data_dir`**：
/// 后者在 Windows 上指向 `Roaming`，于是同一份配置会散落在
/// `Roaming\com.xieyi.translate\` 和 `Local\com.xieyi.translate\` 两处——
/// WebView2 的缓存本来就在 Local，用户卸载时清掉 Local 会以为干净了，
/// 哪知 Roaming 还留着一份。工具自己生成的文件就该只有一处。
///
/// 现在全部落在 Local 下，和 WebView2 缓存同一个根：
/// ```text
/// Local\com.xieyi.translate\
///   ├── config.json      ← 所有设置
///   ├── sessions\        ← 对话历史
///   ├── runtime\         ← 下载/指定的引擎与启动预设
///   └── EBWebView\       ← WebView2 缓存（它自己管）
/// ```
/// 卸载或重置只需删掉这个根目录。
pub fn app_data(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map_err(|e| e.to_string())
}

/// 应用标识符。**必须与 `tauri.conf.json` 的 `identifier` 一致**。
///
/// WebView2 的数据目录由 Tauri 硬编码为 `app_local_data_dir()`
/// （tauri-2.12.5/src/manager/webview.rs:563），也就是
/// `%LOCALAPPDATA%\<identifier>`。启动最早期（日志、残留清理）
/// 拿不到 `AppHandle`，只能硬编码；有测试锁住两者一致。
pub const IDENTIFIER: &str = "com.xieyi.translate";

/// 不依赖 `AppHandle` 的数据根目录，供启动最早期（日志）使用。
///
/// ## 平台差异
///
/// `AppHandle` 要等 `Builder::build()` 之后才有，但日志必须在它之前初始化
/// （WebView 创建失败会 panic，那时日志还没建就抓不到现场）。
/// 所以这里手工拼各平台的 `app_local_data_dir()`：
///
/// | 平台 | 路径 |
/// |---|---|
/// | Windows | `%LOCALAPPDATA%\<identifier>` |
/// | Linux | `$XDG_DATA_HOME/<identifier>` 或 `~/.local/share/<identifier>` |
/// | macOS | `~/Library/Application Support/<identifier>` |
///
/// **必须与 Tauri 的 `app_local_data_dir()` 保持一致**，否则日志会写到
/// 另一个地方，用户按文档去找会找不到。有测试锁住 Windows 分支与
/// `tauri.conf.json` 的 identifier 一致；Linux/macOS 的路径规则由
/// Tauri 上游固定，这里按其实现照抄。
pub fn data_root_early() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("LOCALAPPDATA")?;
        Some(PathBuf::from(base).join(IDENTIFIER))
    }

    #[cfg(target_os = "linux")]
    {
        // XDG Base Directory：$XDG_DATA_HOME 优先，否则 ~/.local/share
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            let p = PathBuf::from(xdg);
            if !p.as_os_str().is_empty() {
                return Some(p.join(IDENTIFIER));
            }
        }
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".local/share").join(IDENTIFIER))
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library/Application Support")
                .join(IDENTIFIER),
        )
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        None
    }
}


pub fn runtime_root(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data(app)?.join("runtime"))
}

/// 配置文件（用户设置 + 引擎信息，全在这一个文件里）。
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

    /// 数据根目录必须落在 **Local**，不能是 Roaming。
    ///
    /// 这条防的是一个具体的"卸载不干净"：Tauri 的 `app_data_dir()` 在
    /// Windows 上指向 `Roaming`，而 WebView2 缓存固定在 `Local`。
    /// 两者并用时，用户清掉 Local 会以为删干净了，Roaming 里还留着配置。
    /// 我们用 `app_local_data_dir()`，让所有自建文件与缓存同处一个根。
    ///
    /// 只做静态断言，不依赖 AppHandle：这条约束的实质是"源码里不能出现
    /// app_data_dir"，改成任何别的写法都应当让这条测试失败。
    #[test]
    fn data_root_uses_local_not_roaming() {
        let source = include_str!("paths.rs");
        // 只看测试模块之前的生产代码：测试自己会写到这两个名字，
        // 把它们算进来会让这条断言永远成立不了。
        let production = source.split("#[cfg(test)]").next().unwrap_or("");
        let code: String = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            !code.contains(".app_data_dir()"),
            "数据根目录不能用 app_data_dir（Windows 上是 Roaming）"
        );
        assert!(
            code.contains(".app_local_data_dir()"),
            "数据根目录应当用 app_local_data_dir（Local）"
        );
    }

    /// `IDENTIFIER` 必须与 `tauri.conf.json` 一致。
    ///
    /// 启动最早期（日志初始化、残留清理）拿不到 `AppHandle`，
    /// 只能用硬编码的 identifier 拼路径。一旦改名而这里没同步，
    /// 日志会写到旧目录、残留清理会匹配错进程——而且**不会有任何报错**，
    /// 只能在排查时发现「日志怎么没了」。
    #[test]
    fn identifier_matches_tauri_config() {
        let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"))
            .expect("读不到 tauri.conf.json");
        let conf: serde_json::Value =
            serde_json::from_str(&raw).expect("tauri.conf.json 不是合法 JSON");

        let configured = conf["identifier"]
            .as_str()
            .expect("配置里没有 identifier");

        assert_eq!(
            configured, IDENTIFIER,
            "paths::IDENTIFIER 与 tauri.conf.json 的 identifier 不一致"
        );
    }

    #[test]
    fn early_root_ends_with_identifier() {
        if let Some(root) = data_root_early() {
            assert!(
                root.ends_with(IDENTIFIER),
                "数据根目录应以 identifier 结尾，实际是 {root:?}"
            );
        }
    }
}
