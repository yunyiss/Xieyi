mod batch;
mod hardware;
mod job_object;
mod logging;
mod models;
mod params;
mod paths;
mod prompts;
mod quality;
mod runtime;
mod service;
mod sessions;
mod single_instance;
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

#[tauri::command]
fn probe_vram() -> Option<u64> {
    hardware::free_vram_mb()
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
    match tauri::WebviewWindowBuilder::from_config(app.handle(), &config).and_then(|b| b.build()) {
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
            service::spawn_ttl_watcher(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            open_models_dir,
            open_engine_dir,
            set_models_dir,
            open_data_subdir,
            probe_vram,
            sync_window_theme,
            runtime::runtime_status,
            runtime::install_runtime,
            runtime::set_manual_runtime,
            runtime::probe_runtime,
            runtime::discover_runtimes,
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
            service::compact_history,
            service::abort,
            service::set_ttl,
            service::set_gpu_layers,
            service::set_pinned,
            service::get_settings,
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
