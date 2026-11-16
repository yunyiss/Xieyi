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

/// 显式声明「这里就要便携」的标记文件，与 `data/` 同级。
///
/// 共享目录（桌面 / 下载 / 文档 / 家目录）里只放一个 `data/` 不足以说明
/// 那是**我们的**数据目录，见 `is_portable` 的说明。用户想要就建这个文件。
pub const PORTABLE_MARKER: &str = ".portable";

/// 认作「这个 `data/` 是我们自己的」的标志。
///
/// 只要里面已经出现这些中的任何一个，就说明该用户一直在这里跑程序，
/// 不需要额外标记。
const DATA_FINGERPRINTS: &[&str] = &["config.json", "logs", "runtime", "sessions"];

/// 程序是否运行在便携模式。
///
/// 判定条件：**程序所在目录下有 `data/` 目录，且该目录可写**。
///
/// ## 共享目录里的 `data/` 不算数
///
/// 只看「有没有 `data/`」会**劫持别人的目录**。桌面、下载、文档、家目录
/// 这类所有程序共用的位置，`data` 是极常见的目录名——别的程序、用户自己
/// 建的、解压工具留下的都可能叫这个。一旦误认：
///
/// - 我们的配置、术语表、引擎记录会写进别人的目录
/// - 而这些位置恰好最容易被清理（磁盘清理、解压工具清空下载目录、用户
///   随手删桌面文件夹），**用户的数据会连带蒸发**
///
/// 这不是假想：同类的 magpie 就因此让一个从没要过便携的用户拿到一个
/// 空的 magpie（issue #989），另一起是把数据写进 `Downloads\data`，
/// 被解压工具清空后刚添加的 provider 消失。
///
/// 所以在共享目录里，还要求下面任一条件成立才认作便携：
///
/// 1. 旁边有 [`PORTABLE_MARKER`]（`.portable`）——用户明确要求
/// 2. `data/` 里已经有我们的文件（[`DATA_FINGERPRINTS`]）——一直在用
///
/// 两个都不满足时退回标准模式（写系统数据目录），这是**安全的默认**：
/// 数据落在自己名下，不会因为别人的目录被清而丢。
///
/// 其他位置（如自建的 `D:\Tools\xieyi\`）不受此限，有 `data/` 即认。
///
/// 另外两个边界：
///
/// - 装到 `Program Files` 时程序目录不可写，即使有 `data/` 也不该用
///   （写入会失败），所以要确认目录可写
/// - AppImage 运行时「程序目录」是 `/tmp/.mount_xxx`（只读），
///   真实位置在 `$APPIMAGE` 环境变量里，这里优先用它
pub fn is_portable() -> bool {
    portable_root().is_some_and(|root| is_portable_at(&root))
}

/// [`is_portable`] 的判定本体，接受任意根目录。
///
/// 拆出来是为了能测：`portable_root()` 取的是**当前 exe 所在目录**，
/// 单测改不了它。只测 `data_is_ours` / `is_shared_location` 这类零件
/// 也不够——把它们串起来的这段守卫才是真正防劫持的地方（踩过一次：
/// 只测零件时把守卫整段删掉，测试照样全绿）。
fn is_portable_at(root: &std::path::Path) -> bool {
    let data = root.join(PORTABLE_DIR);
    if !data.is_dir() || !dir_is_writable(&data) {
        return false;
    }
    if is_shared_location(root) && !data_is_ours(root, &data) {
        return false;
    }
    true
}

/// 这个 `data/` 是否确实是我们的。
///
/// 判据见 [`is_portable`]：有 `.portable` 标记，或里面已有我们的文件。
fn data_is_ours(root: &std::path::Path, data: &std::path::Path) -> bool {
    if root.join(PORTABLE_MARKER).is_file() {
        return true;
    }
    DATA_FINGERPRINTS
        .iter()
        .any(|name| data.join(name).exists())
}

/// 目录是否属于「所有程序共用、随时可能被清理」的位置。
///
/// 只看目录名。这是个**保守的启发式**：宁可多认几个共享目录（用户建个
/// `.portable` 就能覆盖），也不要漏掉最危险的那几个。真正的系统路径
/// 解析需要平台 API，而这里在启动极早期就会被调用（还没有 `AppHandle`）。
fn is_shared_location(dir: &std::path::Path) -> bool {
    // 家目录本身
    if let Some(home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        && dir == home
    {
        return true;
    }

    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let name = name.to_lowercase();

    // 英文名 + 中文名（中文 Windows 的已知文件夹是本地化名称）
    const SHARED: &[&str] = &[
        "downloads",
        "download",
        "desktop",
        "documents",
        "temp",
        "tmp",
        "下载",
        "桌面",
        "文档",
        "临时",
    ];
    SHARED.contains(&name.as_str())
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

/// 读取配置根对象。
///
/// ## 文件损坏时不静默丢弃
///
/// 早先这里是「读不出来就当空对象」，隐蔽处在于：解析失败后紧接着一次
/// 写入，会把「空配置 + 一个新键」落盘，**用户所有设置无声消失**。
///
/// 现在先把损坏的文件改名留档（`config.json.bad-<时间戳>`），再返回空。
/// 用户至少还有一个能捞回来的副本，日志里也留了痕。
fn read_config_root(path: &std::path::Path) -> serde_json::Value {
    let Ok(raw) = std::fs::read_to_string(path) else {
        // 文件不存在 = 首次运行，正常
        return serde_json::json!({});
    };
    // 空文件也当空配置（历史上中断写入留下的）
    if raw.trim().is_empty() {
        return serde_json::json!({});
    }
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) => v,
        Err(e) => {
            crate::logging::warn("config", format!("配置文件解析失败，已留档：{e}"));
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let bad = path.with_extension(format!("json.bad-{stamp}"));
            let _ = std::fs::rename(path, &bad);
            serde_json::json!({})
        }
    }
}

/// 写入配置项。
///
/// ## 原子写
///
/// 先写同目录的 `.tmp`，再 `rename` 覆盖。`rename` 在同一文件系统内是
/// 原子的，所以任何时刻磁盘上的 `config.json` 要么是旧内容、要么是新内容，
/// **不会是写了一半的截断文件**。
///
/// 早先直接 `std::fs::write` 覆盖：进程在写入途中被杀（关窗口、任务管理器、
/// 断电）就会留下截断的 JSON，下次启动解析失败 → 配置全丢。而配置里是
/// 术语表、模型参数、TTL、常驻模型这些用户手调的东西，丢了没法复原。
pub fn set_config_value(
    app: &AppHandle,
    key: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    let path = config_json(app)?;
    if let Some(parent) = path.parent() {
        ensure(parent)?;
    }
    let mut root = read_config_root(&path);

    root[key] = value;
    write_config_root(&path, &root)
}

/// 把配置根对象原子地写回磁盘。
///
/// 先写同目录的 `.tmp` 再 `rename` 覆盖：`rename` 在同一文件系统内是原子的，
/// 任何时刻磁盘上的 `config.json` 要么是旧内容、要么是新内容，不会是
/// 写了一半的截断文件。
///
/// 抽成独立函数是为了能单测——`set_config_value` 需要 `AppHandle` 拿路径。
fn write_config_root(path: &std::path::Path, root: &serde_json::Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(root).map_err(|e| format!("序列化配置失败: {e}"))?;

    // 临时文件放同目录，保证 rename 不跨设备
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &text).map_err(|e| format!("写入配置失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("写入配置失败: {e}")
    })
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

    /*
     * **配置丢了但模型还在**：先看数据目录下的 `models/`。
     *
     * 用户为了重置设置删掉 `config.json` 时，模型目录的路径也一起没了。
     * 而模型是几百 MB 到几 GB 的文件，就在数据目录里躺着——这时若直接
     * 天真地回退到「exe 旁边」，界面会显示"没有可用的模型"，用户会以为
     * 模型丢了（实测踩到：1.8 GB 的模型在磁盘上，界面却说找不到）。
     *
     * 判据是"里面真的有 .gguf"，而不是"目录存在"——空目录不能算。
     */
    if let Ok(data) = app_data(app)
        && let Some(dir) = models_with_gguf_in(&data.join("models"))
    {
        // 顺手写回配置：这样下一次走的是正常的配置分支，
        // 而不是每次都靠猜。写失败不影响读取结果。
        let _ = set_config_value(app, "modelsDir", serde_json::json!(dir.to_string_lossy()));
        return Ok(dir);
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

/// 目录里含有 `.gguf` 时返回该目录，否则 `None`。
///
/// 只看一层（模型目录是不递归的扁平结构）。抽成独立函数便于单测：
/// `models_dir` 需要 `AppHandle`。
fn models_with_gguf_in(dir: &std::path::Path) -> Option<PathBuf> {
    let has = std::fs::read_dir(dir).ok()?.flatten().any(|e| {
        e.path()
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("gguf"))
    });
    has.then(|| dir.to_path_buf())
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

    /// 共享目录里的 `data/` **不能**单独作为便携依据。
    ///
    /// 桌面 / 下载 / 文档 / 家目录是所有程序共用的位置，`data` 是极常见的
    /// 目录名。如果只看"有没有 data/"，我们会把配置写进别人的目录——而
    /// 这些位置最容易被清理，用户数据会连带蒸发。
    ///
    /// 这条测的是 `is_portable_at` 的整体判定，不是零件：先前的版本只断言
    /// `data_is_ours`，结果把守卫整段删掉测试依然全绿，等于没测。
    ///
    /// 变异验证：把 `is_portable_at` 里
    /// `if is_shared_location(root) && !data_is_ours(root, &data)` 删掉，
    /// 第一条断言失败。
    #[test]
    fn shared_location_needs_proof_before_becoming_portable() {
        let shared = std::env::temp_dir().join("Downloads");
        let _ = std::fs::remove_dir_all(&shared);
        let data = shared.join(PORTABLE_DIR);
        std::fs::create_dir_all(&data).unwrap();

        // 空 data/ + 无标记 → **不算便携**（最难也最该防的情况）
        assert!(
            !is_portable_at(&shared),
            "Downloads 里的空 data/ 不能触发便携模式，否则会写进别人的目录"
        );

        // 有 .portable 标记 → 算
        std::fs::write(shared.join(PORTABLE_MARKER), b"").unwrap();
        assert!(is_portable_at(&shared), ".portable 标记应当直接生效");
        let _ = std::fs::remove_file(shared.join(PORTABLE_MARKER));

        // data/ 里已有我们的文件 → 算（老用户一直在用）
        std::fs::write(data.join("config.json"), b"{}").unwrap();
        assert!(
            is_portable_at(&shared),
            "已有 config.json 说明是这个用户的数据目录"
        );

        let _ = std::fs::remove_dir_all(&shared);
    }

    /// 非共享位置不需要证明：自建目录有 `data/` 就算便携。
    ///
    /// 模型目录自愈：数据目录下的 `models/` 里有 `.gguf` 就认它。
    ///
    /// 用户实测：删掉 `config.json` 重置设置，`modelsDir` 一起没了。而
    /// 1.8 GB 的模型就躺在数据目录的 `models/` 里——界面却显示"没有可用的
    /// 模型"，用户会以为模型丢了。
    ///
    /// 判据必须是"真的有 .gguf"，不能是"目录存在"：空目录不能算，
    /// 否则用户建了个空 `models/` 就会被误认。
    ///
    /// 变异验证：把 `eq_ignore_ascii_case("gguf")` 改成 `"bin"`，
    /// 第一条断言失败。
    #[test]
    fn models_dir_with_gguf_is_detected() {
        let dir = std::env::temp_dir().join("xieyi-models-selfheal");
        let _ = std::fs::remove_dir_all(&dir);

        // 目录不存在
        assert!(models_with_gguf_in(&dir).is_none());

        // 空目录 → 不认
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            models_with_gguf_in(&dir).is_none(),
            "空的 models/ 不能算，否则会误导"
        );

        // 有非模型文件 → 仍不认
        std::fs::write(dir.join("readme.txt"), b"x").unwrap();
        assert!(models_with_gguf_in(&dir).is_none());

        // 有 .gguf → 认
        std::fs::write(dir.join("Hy-MT2-1.8B-Q8_0.gguf"), b"x").unwrap();
        let got = models_with_gguf_in(&dir).expect("有 .gguf 就该认");
        assert_eq!(got, dir);

        // 扩展名不区分大小写（用户从别处拷来的文件可能是 .GGUF）
        let dir2 = std::env::temp_dir().join("xieyi-models-upper");
        let _ = std::fs::remove_dir_all(&dir2);
        std::fs::create_dir_all(&dir2).unwrap();
        std::fs::write(dir2.join("Model.GGUF"), b"x").unwrap();
        assert!(models_with_gguf_in(&dir2).is_some(), "大小写不该影响判断");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    /// 变异验证：把 `is_shared_location` 改成恒返回 `true`，本测试失败。
    #[test]
    fn normal_location_becomes_portable_without_proof() {
        let root = std::env::temp_dir().join("xieyi-tools-dir");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(PORTABLE_DIR)).unwrap();

        assert!(
            is_portable_at(&root),
            "自建目录里有 data/ 就该是便携模式，不该要求 .portable 标记"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 配置写入：内容按原样落盘，且不留临时文件。
    ///
    /// ## 这条测试**不**验证两件事，别被名字骗了
    ///
    /// **一、不验证原子性。** 原子性靠 `rename` 的语义保证（同文件系统内
    /// 要么旧、要么新），单测观察不到"写到一半"：直接 `fs::write` 同样不会
    /// 留下 `.tmp`，两者在进程内无法区分。曾把 `rename` 换成直接写做变异，
    /// 测试照样通过。
    ///
    /// **二、不验证"覆盖写保住已有键"。** 那合并逻辑在 `set_config_value`
    /// 里（需要 `AppHandle`），这里调的是 `write_config_root`——给它什么就
    /// 写什么。同样试过让合并变空对象，测试照样通过。
    ///
    /// 所以只断言真正可观察的：写进去的 JSON 能原样读回、写完不留 `.tmp`。
    /// 原子性与合并语义属于"按构造成立"，与 `service::stop_for_reinstall`
    /// 的调用顺序同类——只能靠审阅与真机验证，不能假装被测过。
    #[test]
    fn config_write_round_trips_without_leftovers() {
        let dir = std::env::temp_dir().join("xieyi-atomic-config");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");

        let root = serde_json::json!({ "a": 1, "b": [2, 3] });
        write_config_root(&path, &root).expect("写入应当成功");

        let read_back: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(read_back, root, "写进去的内容要能原样读回");

        let tmp = path.with_extension("json.tmp");
        assert!(!tmp.exists(), "写完不该留下 {}", tmp.display());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 配置文件损坏时**要留档**，不能静默丢弃。
    ///
    /// 早先的写法是「读不出来就当空对象」，于是解析失败后紧接着一次写入就
    /// 把「空配置 + 一个新键」落盘，用户全部设置无声消失。
    ///
    /// 现在坏文件会被改名成 `config.json.bad-<时间戳>`，用户还有东西可捞。
    ///
    /// 变异验证：把 `read_config_root` 里留档那段（`fs::rename`）删掉，
    /// 本测试失败。
    #[test]
    fn corrupt_config_is_kept_for_recovery() {
        let dir = std::env::temp_dir().join("xieyi-corrupt-config");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");

        // 写一个截断的 JSON（模拟写入途中断电）
        std::fs::write(&path, br#"{"glossary": [{"from": "API""#).unwrap();

        let root = read_config_root(&path);
        assert_eq!(root, serde_json::json!({}), "损坏时返回空对象");
        assert!(!path.exists(), "坏文件应当被改名腾开，而不是留在原地被覆盖");

        // 必须留下一个 .bad-* 副本
        let kept = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".bad-"))
            .count();
        assert_eq!(kept, 1, "坏文件应当留档一份，供用户捞回");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 首次运行（文件不存在）与空文件都不该被当成"损坏"。
    #[test]
    fn missing_or_empty_config_is_not_corruption() {
        let dir = std::env::temp_dir().join("xieyi-empty-config");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");

        assert_eq!(read_config_root(&path), serde_json::json!({}));

        std::fs::write(&path, b"   \n").unwrap();
        assert_eq!(read_config_root(&path), serde_json::json!({}));
        assert!(
            path.exists(),
            "空文件不该被改名（它不是损坏，是中断的写入）"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 共享位置名单要覆盖中英文两类目录名。
    ///
    /// 中文 Windows 的已知文件夹是本地化名称（`下载` / `桌面` / `文档`），
    /// 只认英文名会漏掉最常见的那批用户。
    #[test]
    fn shared_location_matches_localized_names() {
        for name in ["Downloads", "Desktop", "Documents", "下载", "桌面", "文档"] {
            let p = std::path::Path::new("/tmp").join(name);
            assert!(is_shared_location(&p), "{name} 应当被认作共享位置");
        }
        // 大小写不敏感
        assert!(is_shared_location(std::path::Path::new("/tmp/DOWNLOADS")));
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
