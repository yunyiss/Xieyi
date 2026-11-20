mod batch;
mod cache;
mod download_gate;
mod downloader;
mod engine_log;
mod hardware;
mod job_object;
mod logging;
mod models;
mod params;
mod partial;
mod paths;
mod presets;
mod prompts;
mod quality;
mod runtime;
mod service;
mod sessions;
mod single_instance;
mod sources;
mod titlebar;
mod translate;

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    os: String,
    arch: String,
    version: String,
    data_dir: String,
    models_dir: String,
    /// 用户是否**显式设过**模型目录。
    ///
    /// `models_dir` 永远有值（配置或程序目录下的默认 `models`），
    /// 不能用它判断「用户是否做过选择」。
    /// 引导层需要这个区分：否则引擎一装好就被当成「配置完成」，
    /// 而模型列表还是空的。
    models_dir_configured: bool,
    runtime_version: Option<String>,
    /// 是否运行在便携模式。
    ///
    /// 界面据此显示「便携模式」/ 「标准模式」标签。
    /// 没有这个标签时，用户把 exe 拖到桌面后会发现设置全没了，
    /// 却不知道原因是「不在便携目录里了」。
    portable: bool,
}

#[tauri::command]
fn app_info(app: tauri::AppHandle) -> Result<AppInfo, String> {
    let data_dir = paths::app_data(&app)?;
    let models_dir = paths::models_dir(&app)?;
    let status = runtime::status(&app);

    Ok(AppInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        portable: paths::is_portable(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: data_dir.to_string_lossy().to_string(),
        models_dir: models_dir.to_string_lossy().to_string(),
        models_dir_configured: paths::config_value(&app, "modelsDir")
            .and_then(|v| v.as_str().map(|s| !s.is_empty()))
            .unwrap_or(false),
        runtime_version: status.version,
    })
}

#[tauri::command]
/// 在系统文件管理器里打开模型目录。
///
/// 注意这里必须**真的去打开**：早先的实现只是把路径返回给前端，
/// 前端拿到的字符串没人用，于是"打开"按钮点了毫无反应。
fn open_models_dir(app: tauri::AppHandle) -> Result<String, String> {
    let dir = paths::models_dir(&app)?;
    paths::ensure(&dir)?;
    let path = dir.to_string_lossy().to_string();

    // 用 opener 插件交给系统处理：Windows 上是资源管理器。
    // 打不开时把原因带回去，别让用户面对一个没反应的按钮。
    tauri_plugin_opener::open_path(&path, None::<&str>)
        .map_err(|e| format!("无法打开目录 {path}：{e}"))?;

    Ok(path)
}

/// 打开**当前引擎所在目录**。
///
/// 与 `open_data_subdir("runtime")` 的区别：那是"下载的引擎会放这里"的数据子目录，
/// 而用户手动指定 D 盘的 llama-server 时，真正该打开的是 D 盘那个目录。
/// 不接受任意路径——路径取自配置里已记录的引擎，前端无法指定要打开哪。
#[tauri::command]
fn open_engine_dir(app: tauri::AppHandle) -> Result<String, String> {
    let info = runtime::read_info(&app).ok_or("尚未配置推理引擎")?;
    let exe = std::path::PathBuf::from(&info.server_path);
    let dir = exe
        .parent()
        .ok_or_else(|| format!("无法确定引擎所在目录: {}", info.server_path))?;
    let path = dir.to_string_lossy().to_string();

    tauri_plugin_opener::open_path(&path, None::<&str>)
        .map_err(|e| format!("无法打开目录 {path}：{e}"))?;
    Ok(path)
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

/// 在系统文件管理器里打开数据目录下的某个子目录。
///
/// 只允许白名单里的几个名字，不接受任意路径——前端能调这个命令，
/// 要防止它被当成"打开任意目录"的通用能力。
#[tauri::command]
fn open_data_subdir(app: tauri::AppHandle, name: String) -> Result<String, String> {
    let sub = match name.as_str() {
        "root" => String::new(),
        "sessions" => "sessions".to_string(),
        "runtime" => "runtime".to_string(),
        "logs" => "logs".to_string(),
        "cache" => "cache".to_string(),
        _ => return Err(format!("不支持的目录: {name}")),
    };

    let base = paths::app_data(&app)?;
    let dir = if sub.is_empty() {
        base
    } else {
        base.join(&sub)
    };
    paths::ensure(&dir)?;
    let path = dir.to_string_lossy().to_string();

    tauri_plugin_opener::open_path(&path, None::<&str>)
        .map_err(|e| format!("无法打开目录 {path}：{e}"))?;
    Ok(path)
}

/// `async`：这个命令会起 `llama-server --list-devices` 与 `nvidia-smi`
/// 两个子进程。Tauri 的同步命令默认在主线程执行，设置抽屉轮询它时
/// 会不定时卡住界面；交给 async runtime 后只占用后台线程。
#[tauri::command(async)]
fn probe_vram(app: tauri::AppHandle) -> Option<u64> {
    // 优先问引擎自己：它对 AMD / Intel / Metal 都能报，
    // 不像 nvidia-smi 只认 NVIDIA 一家。
    hardware::free_vram_mb(engine_server_path(&app).as_deref())
}

/// 硬件总显存（MB），**不依赖已安装的引擎**。
///
/// 与 `probe_vram` 的分工：
///
/// - `probe_vram` → 当前**可用**多少，要引擎在磁盘上（引导层用不上）
/// - 这个 → 硬件**总共**多少，任何时候都能问（预设卡片判断"跑不跑得动"）
/// `async`：读注册表 / sysfs，跨平台实现可能较慢，不占主线程。
#[tauri::command(async)]
fn probe_total_vram() -> Option<u64> {
    hardware::total_vram_mb()
}

/// 内存总量 / 可用量，一次返回两个值。
///
/// 界面上的内存条需要「总 − 可用」；分成两个命令会让两个数字来自
/// 不同时刻。`async` 的取舍见 `probe_vram`。
#[tauri::command(async)]
fn probe_ram() -> Option<hardware::MemoryInfo> {
    hardware::memory_mb()
}

/// 物理总内存（MB）。判定「模型跑不跑得动」时与显存一起看。
///
/// 显存放不下时可以有一部分层留在内存里，只看显存会把这种情况误判成跑不动。
/// 三平台都走 `sysinfo` 的进程内调用，不再起 PowerShell。
#[tauri::command(async)]
fn probe_total_ram() -> Option<u64> {
    hardware::total_ram_mb()
}

/// 当前配置的推理引擎可执行文件路径。没配置时返回 `None`。
fn engine_server_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    runtime::read_info(app)
        .map(|i| std::path::PathBuf::from(i.server_path))
        .filter(|p| p.is_file())
}

/// 让系统窗口（标题栏）跟随界面主题。
///
/// 界面切成深色后标题栏仍是白的会很突兀。Tauri 的 `set_theme` 底层
/// 已封装三平台：Windows 用 DWM、Linux 用 `gtk-application-prefer-dark-theme`、
/// macOS 用 `NSAppearance`。
///
/// `theme` 取 `"light"` / `"dark"` / `"system"`。**`system` 会映射成
/// `None`**（跟随系统），而不是解析成具体颜色——否则 WebView 的
/// `prefers-color-scheme` 会被锁定，前端就没法响应系统主题变化了。
#[tauri::command]
fn sync_window_theme(window: tauri::WebviewWindow, theme: String) {
    titlebar::set_window_theme(&window, &theme);
}

/// 启动最早期的日志初始化：不依赖 `AppHandle`。
///
/// 级别随构建类型走：debug 记全量，release 只记 Info 以上。
/// 不暴露成用户设置——多一个开关就多一处要解释。
fn init_early_logging() {
    let level = if cfg!(debug_assertions) {
        logging::Level::Debug
    } else {
        logging::Level::Info
    };
    if let Some(root) = paths::data_root_early() {
        logging::init(root.join("logs"), level);
    }
}

/// 便携模式下把 webview 的数据目录挪进 `data/`。
///
/// ## 为什么必须有这一步
///
/// "便携模式"的承诺是「文件夹拷走 = 数据全带走」。但两个平台都会把
/// webview 的缓存与 localStorage 写到**系统标准位置**，与我们的 `data/`
/// 无关——于是便携是假的：拷走文件夹，界面状态这些仍然留在原机器上。
///
/// 实测确认过两个平台都有这个缺口：
///
/// - Windows：`%LOCALAPPDATA%\com.xieyi.translate\EBWebView` 确实存在
/// - Linux：`~/.local/share/com.xieyi.translate/WebKitCache` 确实存在
///
/// ## 为什么必须在最早期设置
///
/// 两条系统接口都只在**第一次初始化 webview 时**读环境变量，之后再改无效：
///
/// | 平台 | 变量 | 读它的时机 |
/// |---|---|---|
/// | Windows | `WEBVIEW2_USER_DATA_FOLDER` | 建第一个 WebView2 环境时 |
/// | Linux | `XDG_DATA_HOME` / `XDG_CACHE_HOME` | WebKitGTK 初始化时 |
///
/// 所以只能在 `Builder::new()` 之前调用，`setup` 回调里已经太晚。
///
/// 非便携模式什么都不做，保持系统默认位置（那时它本该在用户目录）。
fn setup_webview_data_dir() {
    // WebView2 只存在于 Windows
    #[cfg(target_os = "windows")]
    {
        let Some(data) = paths::portable_data_dir() else {
            return;
        };
        let webview = data.join("EBWebView");
        if let Err(e) = std::fs::create_dir_all(&webview) {
            // 建不出来就别设：设了但写不进去，WebView2 会直接起不来，
            // 那比缓存落在用户目录严重得多。
            logging::warn(
                "app",
                format!("便携模式的 WebView2 目录不可用，回退到系统位置：{e}"),
            );
            return;
        }
        // SAFETY: 这里是单线程早期阶段（Tauri 尚未启动任何线程），
        // 不存在并发读写环境变量。
        unsafe {
            std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &webview);
        }
        logging::info(
            "app",
            format!("便携模式：WebView2 数据目录 {}", webview.display()),
        );
    }

    /*
     * Linux：WebKitGTK 按 XDG Base Directory 规则找缓存与数据。
     *
     * 这里**只改这两个 XDG 变量，不动 HOME**——HOME 一动，用户的
     * shell 配置、其他程序的数据定位都会跟着变，那是越界。
     *
     * 两个变量分开设：
     * - `XDG_DATA_HOME` → localStorage / IndexedDB / 会话存储
     * - `XDG_CACHE_HOME` → WebKitCache（实测这个目录最大）
     */
    #[cfg(target_os = "linux")]
    {
        let Some(data) = paths::portable_data_dir() else {
            return;
        };
        let cache = data.join("cache");
        for dir in [&data, &cache] {
            if let Err(e) = std::fs::create_dir_all(dir) {
                logging::warn("app", format!("便携模式目录不可用，回退到系统位置：{e}"));
                return;
            }
        }
        // SAFETY: 同 Windows 分支，单线程早期阶段。
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &data);
            std::env::set_var("XDG_CACHE_HOME", &cache);
        }
        logging::info("app", format!("便携模式：XDG 数据目录 {}", data.display()));
    }
}

/// 给 WebView2 进程附加的启动参数。
///
/// ## 为什么要关掉这几个组件
///
/// WebView2 默认会把 Chrome 的全套组件下载下来，其中包括本工具
/// 完全用不到的：
///
/// | 组件 | 默认体积 | 本工具需要吗 |
/// |---|---|---|
/// | WidevineCdm（DRM 解密） | ~22 MB | 不需要：不播放受保护内容 |
/// | Subresource Filter（广告过滤规则） | ~13 MB | 不需要：界面是本地资源 |
/// | Speech Recognition（语音识别） | ~2.6 MB | 不需要：没有语音入口 |
///
/// ## 一旦调用 additional_browser_args，就会覆盖 wry 的默认值
///
/// Tauri 文档明确警告：wry 默认会传
/// `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`，
/// 我们自己指定后必须把这三个也写上，否则它们会重新启用。
pub const WEBVIEW_ARGS: &str = concat!(
    // 保留 wry 的三个默认禁用项
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,",
    // 本工具额外禁用的
    "MediaRouter,WidevineCdm,",
    "msWebView2EnableShoppingFeatures,msWebView2EnableFamilySafety",
    // 关掉后台组件更新检查：本地工具无需跟随 Edge 更新组件
    " --disable-component-update",
    // 不做首次运行向导（那是浏览器的玩意）
    " --no-first-run --no-default-browser-check",
);

/// 把浏览器参数应用到建窗构造器。
///
/// 单独抽成函数是为了让「传了什么」可以被测试断言，
/// 而不必真去建一个窗口。
///
/// ## 为什么便携模式下也不搬 WebView 缓存
///
/// 便携模式只搬**我们自己的数据**（配置、日志、会话、缓存、引擎）。
/// WebView2 的缓存目录保持 Tauri 默认位置（`app_local_data_dir`）。
///
/// 理由：
/// - 它纯粹是界面缓存，删掉程序重开就会重建，搬走没有实际收益
/// - 跨平台差异大：macOS 的 WKWebView 根本不支持自定义数据目录，
///   只有 `data_store_identifier`（16 字节标识，不是路径），
///   强行统一会让三端行为不一致
/// - 卸载时清两处（程序目录 + AppData）本来就是常规操作，
///   多这一层不会带来困扰
fn apply_webview_args(
    builder: tauri::WebviewWindowBuilder<'_, tauri::Wry, tauri::AppHandle>,
) -> tauri::WebviewWindowBuilder<'_, tauri::Wry, tauri::AppHandle> {
    builder.additional_browser_args(WEBVIEW_ARGS)
}

/// 创建主窗口，失败时清理残留并重试。
///
/// ## 为什么要重试
///
/// WebView2 的数据目录可能被**上一次崩溃留下的进程**占着，报
/// `ERROR_BUSY`。这种情况等一会儿、把残留清掉就能起来，
/// 没必要让用户看到错误框。
///
/// 退避 200 / 500 / 1000ms，最多 3 次。**清理只在失败路径执行**——
/// 正常启动不跑 PowerShell，也就没有那 ~100ms 的额外开销。
fn create_main_window(app: &tauri::App) -> Result<(), String> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .ok_or_else(|| "配置里找不到 label 为 main 的窗口".to_string())?
        .clone();

    // 只剩一次机会：预检和清理已经在 `run()` 里做完了。
    //
    // **为什么不在 setup 里重试**：setup 跑在事件循环的 Ready 事件里，
    // 在里面 sleep / 起 PowerShell 会把这个线程卡住，实测第一次失败后
    // 进程直接没了（日志只留下第 0 次那条 WARN）。
    // 重试与退避必须放在**事件循环启动之前**。
    match tauri::WebviewWindowBuilder::from_config(app.handle(), &config)
        .and_then(|b| apply_webview_args(b).build())
    {
        Ok(window) => {
            // 建出窗口对象 ≠ 上了屏幕。
            //
            // 2026-11-05 在一台 x86 黑苹果上实测：`build()` 返回 Ok，
            // `is_visible()` 为 true，位置尺寸都正常，但 CGWindowList
            // 里查不到该窗口、截图上也只有壁纸。那是那台 VM 的
            // WindowServer 合成不可信，不是代码问题——但没有这几行
            // 日志时完全无法区分「建窗失败」和「建窗成功但没显示」，
            // 排查多花了半小时。
            //
            // 保留为 info 级：正常启动也就 4 行，不影响日志可读性。
            crate::logging::info("window", "主窗口已创建");
            // **建窗后立刻上底色**，不要等前端 syncWindowTheme。
            //
            // WebView2 从开始加载到 React 挂载完成这段时间，页面里什么都
            // 没有（`index.html` 的 `#root` 是空的，CSS 由 JS 引入），
            // 显示的是 WebView2 默认的**白底**。窗口又早已可见（实测
            // 「可见性 true」就在建窗那一秒），于是用户看到一块白板。
            //
            // 先铺上和 `--canvas` 一致的底色，白板就变成了"和最终界面
            // 同色的一闪"。不改变总时长，但视觉上不再是刺眼的白。
            crate::titlebar::set_window_background(&window, "system");
            if let Ok(pos) = window.outer_position() {
                crate::logging::info("window", format!("窗口位置 {pos:?}"));
            }
            if let Ok(size) = window.outer_size() {
                crate::logging::info("window", format!("窗口尺寸 {size:?}"));
            }
            match window.is_visible() {
                Ok(v) => crate::logging::info("window", format!("可见性 {v}")),
                Err(e) => crate::logging::warn("window", format!("读可见性失败: {e}")),
            }
            if let Err(e) = window.set_focus() {
                crate::logging::warn("window", format!("聚焦失败: {e}"));
            }
            Ok(())
        }
        Err(e) => {
            crate::logging::error("window", format!("建窗失败: {e}"));
            Err(e.to_string())
        }
    }
}

/// 启动失败时：写日志 + 弹原生中文错误框，然后退出。
///
/// **绝不允许再走到 panic/abort**。Tauri 默认对 setup 错误直接
/// `panic!`（app.rs:1444），在 `panic = "abort"` 下进程以
/// `0xc0000409` 静默消失——用户只看到一个白窗口闪一下，什么都得不到。
fn report_startup_failure(_app: &tauri::AppHandle, err: &str) {
    let data_root = paths::data_root_early()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "%LOCALAPPDATA%\\com.xieyi.translate".to_string());
    let log_dir = format!("{data_root}\\logs");

    logging::error("app", format!("启动失败: {err}"));

    let busy = err.contains("0x800700AA") || err.contains("资源在使用中");
    let detail = if busy {
        "WebView2 的数据目录正被其他进程占用。\n\n\
         通常是上一次异常退出留下的残留。可以：\n\
         · 稍等几秒再试\n\
         · 在任务管理器里结束所有 msedgewebview2.exe\n\
         · 仍不行就重启一次电脑"
    } else {
        "创建界面失败。详细信息已写入日志。"
    };

    show_error_dialog(&format!(
        "Xieyi 写意 启动失败\n\n{detail}\n\n错误信息：{err}\n\n日志目录：{log_dir}"
    ));
}

/// 弹一个原生错误框。用 `user32!MessageBoxW`，不为此引依赖。
fn show_error_dialog(text: &str) {
    #[cfg(windows)]
    {
        show_error_dialog_windows(text);
    }
    #[cfg(not(windows))]
    {
        eprintln!("{text}");
    }
}

#[cfg(windows)]
fn show_error_dialog_windows(text: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, flags: u32) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    const MB_ICONERROR: u32 = 0x0000_0010;
    const MB_OK: u32 = 0x0000_0000;

    let text = wide(text);
    let caption = wide("Xieyi 写意");
    unsafe {
        MessageBoxW(0, text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // ── 日志前置 ────────────────────────────────────────────────────
    // 必须在 `Builder::new()` 之前：窗口和 WebView 是在 `build()` 内部
    // 创建的（tauri-2.12.5/src/app.rs:2692），失败时直接 `panic!`。
    // 如果把日志初始化放在 setup 回调里，那条 panic 发生时日志还没建，
    // 用户和我们都看不到任何记录——这正是之前"日志目录空白"的原因。
    //
    // 此处没有 `AppHandle`，所以路径用 `paths::data_root_early()`
    // （硬编码 identifier，有测试与 tauri.conf.json 对齐）。
    init_early_logging();

    logging::info("app", format!("启动 Xieyi v{}", env!("CARGO_PKG_VERSION")));

    // ── WebView2 数据目录：便携模式必须跟着走 ───────────────────────
    // 必须在本进程创建任何 WebView **之前**设置：WebView2 加载器在
    // 首次建环境时读这个变量，之后再改无效。
    //
    // 早先只在文档和注释里画了 `data\EBWebView\`，代码里从没设过它。
    // 结果是"便携模式"并不便携：配置、引擎、模型都进了 `data/`，
    // 而 WebView2 的缓存、localStorage 仍然写在
    // `%LOCALAPPDATA%\com.xieyi.translate\EBWebView`（实测确认存在）。
    // 用户以为把文件夹拷走就带走了全部数据，其实没有。
    setup_webview_data_dir();

    // ── 单实例：必须先于任何 Tauri 代码 ─────────────────────────────
    // `tauri-plugin-single-instance` 的互斥量在 setup 里才检查，
    // 而 WebView 在 `build()` 内部就已创建。两个实例同时启动时
    // 都走到创建 WebView，互相撞 `EBWebView` 目录，**双双 panic**。
    //
    // 放在日志初始化**之后**：第二实例的聚焦失败需要留痕，
    // 否则用户只会看到「点了图标没反应」，无从排查。
    let _guard = match single_instance::acquire() {
        Ok(g) => g,
        Err(()) => {
            // 已有实例，窗口已聚焦（失败会记 warn），静默退出
            std::process::exit(0);
        }
    };

    tauri::Builder::default()
        // 插件版单实例不再注册：上面的互斥量已经保证唯一。
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(service::AppState::default())
        .setup(|app| {
            // 窗口在配置里设了 `create: false`，所以 Tauri 不会自动建。
            // 这样创建失败的错误落到我们手里，而不是变成 panic。
            if let Err(e) = create_main_window(app) {
                report_startup_failure(app.handle(), &e);
                app.handle().exit(1);
                return Ok(());
            }

            service::load_settings(app.handle());

            // 配置里没有引擎、磁盘上却有 —— 认回来。
            //
            // 用户可能为了"重置设置"删掉 config.json 而留下引擎目录。
            // 不认的话，界面会认为什么都没装，而下载又被"目录已存在"挡住，
            // 卡死在引导层。这里放在启动阶段做一次，探测一次子进程，
            // 不进任何轮询。
            runtime::adopt_existing_engine(app.handle());
            // 磁盘缓存放后台线程装回：解析 JSONL 要几十毫秒，
            // 不该拖慢「双击到看见窗口」。装回来之前翻译只是少命中几次。
            service::load_disk_cache(app.handle());
            service::spawn_ttl_watcher(app.handle().clone());
            // 用户显式开启「启动时预加载」才跑；默认什么都不做。
            service::maybe_preload(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            open_models_dir,
            open_engine_dir,
            set_models_dir,
            open_data_subdir,
            probe_vram,
            probe_total_vram,
            probe_total_ram,
            probe_ram,
            sync_window_theme,
            runtime::runtime_status,
            runtime::install_runtime,
            runtime::set_manual_runtime,
            runtime::discover_runtimes,
            runtime::delete_runtime_partial,
            service::service_status,
            service::start_service,
            service::stop_service,
            service::list_router_models,
            service::load_model,
            service::translate,
            service::get_glossary,
            service::set_glossary,
            service::chat,
            service::compact_history,
            service::abort,
            service::set_ttl,
            service::clear_translate_cache,
            service::cache_status,
            service::set_gpu_layers,
            service::set_preload,
            service::set_preload_model,
            service::set_pinned,
            service::get_settings,
            sources::get_download_source_config,
            sources::set_download_source_config,
            presets::list_presets,
            presets::download_preset,
            download_gate::cancel_download,
            presets::delete_preset_partial,
            service::count_local_models,
            service::list_local_models,
            service::set_model_params,
            service::set_model_chat_capable,
            service::thinking_tiers,
            service::restart_service,
            sessions::list_sessions,
            sessions::save_session,
            sessions::delete_session,
            sessions::delete_all_sessions,
            batch::batch_translate,
            batch::batch_cancel,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // 不再直接 panic：把「资源被占用」这类可自愈的情况翻译成人话。
            //
            // Tauri 的原始报错是这样的，用户完全看不懂：
            //   Failed to setup app: runtime error: failed to create webview:
            //   WebView2 error: WindowsError(Error { code: HRESULT(0x800700AA),
            //   message: "请求的资源在使用中。" })
            let msg = e.to_string();
            let hint = if msg.contains("0x800700AA") || msg.contains("资源在使用中") {
                "WebView2 的数据目录正被其他进程占用。\n\
                 多半是上一次异常退出留下的残留，请稍等几秒重试；\n\
                 若反复出现，注销一次 Windows 账户或重启即可。"
            } else {
                "启动失败。若反复出现，请查看数据目录下的 logs\\ 日志。"
            };

            eprintln!("\n启动失败：{msg}\n\n{hint}\n");
            // 日志已初始化的话也落一份，方便事后查
            logging::error("app", format!("启动失败: {msg}"));
            std::process::exit(1);
        })
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                logging::info("app", "退出，回收推理进程树");
                service::shutdown_blocking(app, std::time::Duration::from_secs(3));
                logging::info("app", "推理进程已回收");
            }
        });
}

#[cfg(test)]
mod tests {
    /// WebView2 启动参数必须保留 wry 的默认禁用项。
    ///
    /// `additional_browser_args` 会**整体覆盖** wry 的默认值。
    /// 漏掉 `msWebOOUI` / `msPdfOOUI` / `msSmartScreenProtection` 中的
    /// 任何一个，对应的组件就会重新启用并下载——用户在磁盘上看到
    /// 多出来的几十 MB，却不知道是哪来的。
    #[test]
    fn webview_args_keep_wry_defaults() {
        for feature in ["msWebOOUI", "msPdfOOUI", "msSmartScreenProtection"] {
            assert!(
                super::WEBVIEW_ARGS.contains(feature),
                "wry 默认禁用的 {feature} 被覆盖掉了，组件会重新下载"
            );
        }
    }

    /// 本工具额外禁用的组件都在参数里。
    #[test]
    fn webview_args_disable_unused_components() {
        for feature in ["WidevineCdm", "MediaRouter"] {
            assert!(
                super::WEBVIEW_ARGS.contains(feature),
                "{feature} 应当被禁用（本工具用不到）"
            );
        }
        assert!(
            super::WEBVIEW_ARGS.contains("--disable-component-update"),
            "后台组件更新会持续下载用不到的东西"
        );
    }

    /// 参数里不能有空格分隔的多个 `--disable-features`。
    ///
    /// Chromium 只认最后一个 `--disable-features`，出现两次会让前一组
    /// 静默失效——这种问题不会报错，只会让组件莫名其妙又回来。
    #[test]
    fn webview_args_have_single_disable_features_flag() {
        let count = super::WEBVIEW_ARGS.matches("--disable-features=").count();
        assert_eq!(
            count, 1,
            "只允许一个 --disable-features（Chromium 只认最后一个），实际 {count} 个"
        );
    }

    /// `focus_main_window` 靠 label `"main"` 找窗口。
    ///
    /// 配置里没写 label 时 Tauri 默认就是 `main`，但只要有人显式改成别的
    /// （或者加了多窗口），单实例的「聚焦已有窗口」就会**静默失效**——
    /// 第二个实例照常退出，看起来一切正常，实际上用户双击图标毫无反应。
    /// 这种 bug 不会报错、不会崩，只能靠测试钉住。
    #[test]
    fn default_window_label_is_main() {
        let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"))
            .expect("读不到 tauri.conf.json");
        let conf: serde_json::Value =
            serde_json::from_str(&raw).expect("tauri.conf.json 不是合法 JSON");

        let windows = conf["app"]["windows"]
            .as_array()
            .expect("配置里没有 app.windows");

        let labels: Vec<&str> = windows
            .iter()
            .map(|w| w["label"].as_str().unwrap_or("main"))
            .collect();

        assert!(
            labels.contains(&"main"),
            "focus_main_window 依赖 label \"main\"，但当前窗口 label 是 {labels:?}；\
             改配置的同时要同步改 focus_main_window"
        );
    }
}
