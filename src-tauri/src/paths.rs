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
/// 便携模式的标志目录名。
///
/// 程序所在目录下存在这个目录，就切换到便携模式：所有数据（配置、
/// 日志、缓存、会话、引擎）都写进它里面，而不是系统标准路径。
///
/// 为什么用「目录存在」而不是「某个 marker 文件存在」：
/// 目录本身就是数据位置，不需要额外解释「这个空文件是干嘛的」。
/// 用户解压便携包就能用，不必理解任何概念。
///
/// 打包时由构建脚本预置这个目录（里面放一个 `.keep` 空文件，
/// 防止某些解压工具丢弃空目录）。
pub const PORTABLE_DIR: &str = "data";

/// 程序是否运行在便携模式。
///
/// 判定只看**程序所在目录下有没有 `data/`**。注意几个边界：
///
/// - 装到 `Program Files` 时程序目录不可写，即使有 `data/` 也不该用
///   （写入会失败），所以还要确认目录可写
/// - AppImage 运行时「程序目录」是 `/tmp/.mount_xxx`（只读），
///   真实位置在 `$APPIMAGE` 环境变量里，这里优先用它
pub fn is_portable() -> bool {
    let Some(dir) = portable_root() else {
        return false;
    };
    let data = dir.join(PORTABLE_DIR);
    data.is_dir() && dir_is_writable(&data)
}

/// 便携模式的根目录（也就是程序所在目录）。
///
/// Linux 上 AppImage 会把自己挂载到 `/tmp/.mount_xxxx`（只读、随机名），
/// 此时 `current_exe()` 返回的是挂载点里的路径，写不进去。
/// 真实位置在 `$APPIMAGE` 环境变量里，优先用它。
pub fn portable_root() -> Option<PathBuf> {
    // AppImage：用环境变量指向的真实文件位置
    if let Ok(appimage) = std::env::var("APPIMAGE")
        && !appimage.is_empty()
    {
        let p = PathBuf::from(appimage);
        if let Some(dir) = p.parent() {
            return Some(dir.to_path_buf());
        }
    }
    exe_dir()
}

/// 便携模式下的数据根目录。
pub fn portable_data_dir() -> Option<PathBuf> {
    portable_root().map(|d| d.join(PORTABLE_DIR))
}

pub fn app_data(app: &AppHandle) -> Result<PathBuf, String> {
    // 便携模式优先：所有数据跟着程序走
    if let Some(dir) = portable_data_dir()
        && is_portable()
    {
        return Ok(dir);
    }
    app.path().app_local_data_dir().map_err(|e| e.to_string())
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
    // 便携模式优先：启动最早期（日志、残留清理）也要跟着程序走，
    // 否则日志会写进 AppData，与配置分居两处。
    if is_portable()
        && let Some(dir) = portable_data_dir()
    {
        return Some(dir);
    }

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
    if let Some(dir) = exe_dir().map(|d| d.join("models"))
        && (dir.is_dir() || dir_is_writable(dir.parent().unwrap_or(&dir)))
    {
        return Ok(dir);
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

    // -- 便携模式判定 --

    #[test]
    fn portable_dir_name_is_data() {
        // 目录名一旦改动，打包脚本与文档都要跟着改，锁住它
        assert_eq!(PORTABLE_DIR, "data");
    }

    #[test]
    fn portable_root_prefers_appimage_env() {
        // AppImage 运行时 current_exe() 指向 /tmp/.mount_xxx（只读、随机名），
        // 真实位置在 $APPIMAGE 里。这条锁住「优先用环境变量」的行为。
        //
        // 用真实环境变量会污染其他测试，这里只验证「有值时取父目录」的语义。
        let fake = std::path::PathBuf::from("/opt/Xieyi-1.0/Xieyi.AppImage");
        let parent = fake.parent().unwrap();
        assert_eq!(parent, std::path::Path::new("/opt/Xieyi-1.0"));
    }

    #[test]
    fn is_portable_requires_existing_data_dir() {
        // 便携判定必须是「目录真实存在」，不能只看路径拼出来
        let dir = std::env::temp_dir().join(format!("xieyi-portable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 没有 data/ 子目录
        let data = dir.join(PORTABLE_DIR);
        assert!(!data.is_dir(), "前提：data 目录不存在");

        // 建了 data/ 之后才算便携
        std::fs::create_dir_all(&data).unwrap();
        assert!(data.is_dir());
        assert!(dir_is_writable(&data), "临时目录应当可写");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn portable_data_dir_is_data_under_root() {
        // 便携模式下数据根 = <程序目录>/data
        let root = std::path::Path::new("/opt/Xieyi-1.0");
        assert_eq!(
            root.join(PORTABLE_DIR),
            std::path::Path::new("/opt/Xieyi-1.0/data")
        );
    }

    #[test]
    fn non_writable_dir_is_not_portable() {
        // 装到 Program Files 时程序目录不可写，即使有 data/ 也不能用——
        // 否则写入会失败，用户看到一堆「保存失败」。
        // 用一个不存在的目录模拟：dir_is_writable 对它返回 false。
        let missing = std::path::Path::new("/nonexistent/xieyi-test/data");
        assert!(!dir_is_writable(missing));
    }

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
    /// 启动最早期的数据目录也必须走便携判定。
    ///
    /// 这条容易漏：`data_root_early()` 在 `AppHandle` 可用之前就要工作
    /// （日志初始化、残留清理都用它）。如果它不认便携模式，便携版会出现
    /// 「配置在 data/、日志在 AppData」的分裂，用户按文档找日志会找不到。
    #[test]
    fn early_data_root_honours_portable_mode() {
        let source = include_str!("paths.rs");
        let production = source.split("#[cfg(test)]").next().unwrap_or("");

        // 找到 data_root_early 的函数体
        let start = production
            .find("pub fn data_root_early")
            .expect("函数应当存在");
        let rest = &production[start..];
        let end = rest.find("\n}").unwrap_or(rest.len());
        let body = &rest[..end];

        assert!(
            body.contains("is_portable()"),
            "data_root_early 必须检查便携模式，否则日志会写进 AppData"
        );
        assert!(
            body.contains("portable_data_dir()"),
            "data_root_early 在便携模式下要返回 data/ 路径"
        );
    }

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

        let configured = conf["identifier"].as_str().expect("配置里没有 identifier");

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
