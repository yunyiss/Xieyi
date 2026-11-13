use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

use crate::downloader;
use crate::paths;
use crate::sources;

/// 固定的 llama.cpp 构建号。路由模式属于较新特性，必须 pin 版本而不是用 latest。
pub const PINNED_VERSION: &str = "b11714";

/// 已核对过的下载包校验值（按平台 + 后端区分）。
///
/// 数据来源：llama.cpp 的 release 页面直接公布了每个 asset 的 sha256，
/// 不用真的下载一遍。改 `PINNED_VERSION` 时必须同步更新这里——
/// 有测试（`pinned_assets_all_have_hashes`）会强制检查。
///
/// **没有记录的组合会跳过校验**，只保存实际哈希。新增后端时记得补。
fn expected_sha256(backend: &str) -> Option<&'static str> {
    // 每个平台段都写成「表达式」而不是「语句 + return」：
    // 用块表达式时整段就是这个分支的值，与有没有 return 无关，
    // 也不会因为某个平台段多了 return 而让 clippy 在别的平台上报警
    // （Windows 上曾因此报 0 条、Linux 上报 2 条，两边结果不一致）。
    #[cfg(target_os = "windows")]
    {
        match backend {
            "cpu" => {
                // llama-b11714-bin-win-cpu-x64.zip
                Some("f78eded047061a3f86372db426797400a2f1bb0ec37868d5927dd820eb3386c0")
            }
            _ => {
                // llama-b11714-bin-win-vulkan-x64.zip
                Some("d4765447307f29fc0b4c8ae5b8b806c46b3e515d0ce5c2a54dd90d9b2c7a392f")
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        match backend {
            "cpu" => {
                // llama-b11714-bin-ubuntu-x64.tar.gz
                Some("639059a1106e706810486285ec6d1d92c961f3f920ef78bbdd1ee359428679d0")
            }
            _ => {
                // llama-b11714-bin-ubuntu-vulkan-x64.tar.gz（发行版无 NVIDIA 驱动时的默认）
                Some("a41bc4a20dc7935492a7a1216aa10c39ff7919349864873a46c74022bcd28a26")
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // llama-b11714-bin-macos-arm64.tar.gz
        //
        // 只支持 Apple Silicon：上游不再为 x86_64 macOS 出 Vulkan 包，
        // 而 Metal 后端在 arm64 上是唯一被验证过的路径。
        let _ = backend;
        Some("063f58b7d8458d89eacb2f28a10a6d6e0efa2a81866690b0f6f8ebac6e3846bb")
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        let _ = backend;
        None
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub version: String,
    pub backend: String,
    pub server_path: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub sha256: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub backend: Option<String>,
    pub server_path: Option<String>,
    pub models_dir: String,
    /// 自检结果：能否实际跑起来、报什么版本。
    ///
    /// 只判断"文件存在"是不够的——用户可能选了个改名后的别的程序，
    /// 或者版本不匹配的 llama-server。这里把 `--version` 的输出带上来，
    /// 界面上能直接看到"这个引擎到底是什么"。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe: Option<RuntimeProbe>,
    /// 有没有**未完成**的引擎包下载残留在磁盘上（字节数）。
    ///
    /// ## 为什么必须有这个字段
    ///
    /// 引擎包下载到 `runtime/downloads/*.zip.part`。取消之后那份数据
    /// 是**故意保留**的（下次续传要用），但它此前完全不可见：
    ///
    /// - 不在任何列表里
    /// - 启动时不扫描
    /// - 界面上没有入口
    ///
    /// 而 31 MB 在 90 KB/s 的国内网络下要下 5 分钟，用户很可能取消过
    /// 好几次，磁盘上就堆了好几份半截包，而他一无所知。
    ///
    /// 这与模型的 `.part` 是**同一个缺陷**。模型那边上一轮已经修了
    /// （`PresetStatus.partialBytes` + 「删除」按钮），引擎这边当时漏了。
    pub partial_bytes: u64,
}

/// 对某个 llama-server 可执行文件做一次自检。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProbe {
    pub ok: bool,
    /// `--version` 输出的第一行（成功时），或失败原因
    pub detail: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    stage: String,
    downloaded: u64,
    total: u64,
    percent: f64,
}

/// 读取当前引擎信息。
///
/// 存在 `config.json` 的 `engine` 字段里，不再单独维护 `runtime/runtime.json`：
/// 一个配置文件就是全部设置，删它即还原，用户和调试者都不用记哪个文件管什么。
pub fn read_info(app: &AppHandle) -> Option<RuntimeInfo> {
    let value = paths::config_value(app, "engine")?;
    serde_json::from_value(value).ok()
}

/// 写入引擎信息（同样是 `config.json` 的 `engine` 字段）。
fn write_info(app: &AppHandle, info: &RuntimeInfo) -> Result<(), String> {
    let value = serde_json::to_value(info).map_err(|e| e.to_string())?;
    paths::set_config_value(app, "engine", value)
}

pub fn status(app: &AppHandle) -> RuntimeStatus {
    let info = read_info(app);
    let models_dir = paths::models_dir(app)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    RuntimeStatus {
        installed: info
            .as_ref()
            .map(|i| Path::new(&i.server_path).exists())
            .unwrap_or(false),
        version: info.as_ref().map(|i| i.version.clone()),
        backend: info.as_ref().map(|i| i.backend.clone()),
        server_path: info.as_ref().map(|i| i.server_path.clone()),
        models_dir,
        // 状态查询要保持轻量：每次 2.5 秒轮询都跑一次 --version 太重，
        // 自检只在"切换引擎"这种显式操作时做。
        probe: None,
        partial_bytes: partial_bytes(app),
    }
}

/// 这个后端的引擎是否已经装在 `runtime/` 下。
///
/// 返回安装目录（含可执行文件），没装返回 `None`。
///
/// ## 为什么按目录判断，而不看配置里的 `backend`
///
/// 用户手动指定引擎时，配置里 `backend` 记作 `manual`，但
/// `runtime/` 下并没有我们下载的那一份。此时仍应允许下载——
/// 他可能想换成我们提供的 Vulkan 版。
///
/// 反过来，配置说装了 vulkan 但用户把目录删了，这里也应当返回
/// `None`（允许重装），因为判断依据是**磁盘上的实际文件**。
fn already_installed(root: &Path, backend: &str) -> Option<String> {
    let dir = root.join(format!("llama-{PINNED_VERSION}-{backend}"));
    find_executable(&dir).map(|_| dir.to_string_lossy().to_string())
}

/// 该不该拦住这次安装。
///
/// 返回 `Some(路径)` 表示拒绝，`None` 表示放行。
///
/// ## 为什么需要 `force`
///
/// 拦住重复安装是为了防止「已经装好了还点下载，白等 31 MB」。但同一个
/// 能力反过来就是**修复手段**：引擎目录被损坏、缺了 DLL、`llama-server`
/// 启动即崩时，重下一遍是最直接的自救方式。
///
/// 设置里的「重新下载」正是这条修复路径，所以它必须能绕过闸门。引导层
/// 那个按钮在装好后会隐藏，走的永远是 `force = false`。
///
/// 抽成独立函数是为了能单测——主流程要 `AppHandle`，测不了。
fn reinstall_blocked(force: bool, root: &Path, backend: &str) -> Option<String> {
    if force {
        // 明确要求重装：不做任何检查
        return None;
    }
    already_installed(root, backend)
}

/// 扫一遍 `runtime/downloads/`，累加所有未完成下载的字节数。
///
/// 引擎包只有一个文件名（`llama-<版本>-<后端>.zip`），所以正常情况下
/// 最多一份残留。用累加而不是取单个，是为了容错：用户改过后端
/// （vulkan → cuda）就会留下另一份，两份都该被算进去并清理。
///
/// 只累加 `.part` 后缀的文件。解压用的完整 zip 在成功后会删掉，
/// 不该出现在这里。
fn partial_bytes(app: &AppHandle) -> u64 {
    let Ok(root) = paths::runtime_root(app) else {
        return 0;
    };
    sum_partials_in(&root.join("downloads"))
}

/// `partial_bytes` 的纯逻辑部分：扫目录、认后缀、累加。
///
/// 抽出来是为了能单测——`AppHandle` 在单测里造不出来，
/// 而"后缀判断"恰恰是这里最容易写错的地方。
fn sum_partials_in(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            e.path()
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".part"))
                .unwrap_or(false)
        })
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// 删掉所有未完成的引擎包下载，返回释放的字节数。
///
/// 与模型的「删除」入口对称：取消是"先停一下"（保留以便续传），
/// 删除是"不要了"。没有这个入口，用户取消几次之后那些半截包
/// 就永远躺在磁盘上，而且没有任何线索能查。
#[tauri::command]
pub fn delete_runtime_partial(app: AppHandle) -> Result<u64, String> {
    // 正在下载时不许删：文件句柄还在写，删了会立刻重新长出来
    if crate::download_gate::is_busy() {
        return Err("请先取消正在进行的下载，再删除已下载的部分".into());
    }

    let root = paths::runtime_root(&app)?;
    let downloads = root.join("downloads");
    let freed = clean_partials_in(&downloads);

    // downloads 空了就收掉，别留一个空目录
    let _ = std::fs::remove_dir(&downloads);
    Ok(freed)
}

/// `delete_runtime_partial` 的纯逻辑部分。
///
/// 先算字节数再删（删完就没法 `metadata` 了），删的时候 `.part` 与
/// `.part.meta` 一起删。
fn clean_partials_in(dir: &Path) -> u64 {
    let freed = sum_partials_in(dir);

    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            let is_partial = p
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".part") || n.ends_with(".part.meta"))
                .unwrap_or(false);
            if is_partial {
                let _ = std::fs::remove_file(&p);
            }
        }
    }

    freed
}

/// 对指定路径的 llama-server 跑一次 `--version` 自检。
///
/// 只判断"文件存在"不够：用户可能选成别的程序，或者版本不匹配。
/// 这里真的去执行一次，把版本行带回来——既是校验，也是给人看的信息。
fn probe_exe(path: &Path) -> RuntimeProbe {
    if !path.exists() {
        return RuntimeProbe {
            ok: false,
            detail: "文件不存在".into(),
        };
    }

    let output = std::process::Command::new(path)
        .arg("--version")
        .creation_flags_no_window()
        .output();

    match output {
        Ok(out) => {
            // llama-server 把版本打在 stdout，个别构建打 stderr
            let text = if out.stdout.is_empty() {
                String::from_utf8_lossy(&out.stderr).to_string()
            } else {
                String::from_utf8_lossy(&out.stdout).to_string()
            };
            let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");

            if out.status.success() && !first.is_empty() {
                RuntimeProbe {
                    ok: true,
                    detail: first.trim().to_string(),
                }
            } else {
                RuntimeProbe {
                    ok: false,
                    detail: format!(
                        "无法执行（{}）。可能不是 llama-server，或缺少 VC++ 运行库",
                        out.status
                    ),
                }
            }
        }
        Err(e) => RuntimeProbe {
            ok: false,
            detail: format!("启动失败：{e}"),
        },
    }
}

/// Windows 上不弹黑框；其他平台是空操作。
trait NoWindow {
    fn creation_flags_no_window(&mut self) -> &mut Self;
}

impl NoWindow for std::process::Command {
    fn creation_flags_no_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        self
    }
}

/// 解析出可执行的 llama-server 路径。
pub fn server_exe(app: &AppHandle) -> Result<PathBuf, String> {
    let info = read_info(app).ok_or("推理运行时尚未安装")?;
    let path = PathBuf::from(&info.server_path);
    if !path.exists() {
        return Err("推理运行时文件缺失，请重新安装".into());
    }
    Ok(path)
}

/// 各平台发布包的扩展名。
///
/// **Windows 是 `.zip`，Linux/macOS 是 `.tar.gz`**——这是 llama.cpp
/// 上游的打包习惯，不是我们能选的。之前这里一律按 `.zip` 处理，
/// 导致 Linux 上拼出的 URL 直接 404。
fn archive_ext() -> &'static str {
    if cfg!(target_os = "windows") {
        "zip"
    } else {
        "tar.gz"
    }
}

/// 解压发布包。Windows 是 zip，Linux/macOS 是 tar.gz。
///
/// tar.gz 的解析交给系统 `tar`：Rust 侧的 `tar` + `flate2` 两个 crate
/// 会让产物再涨几百 KB，而我们只需要解一个自己下载的、结构固定的包。
/// 系统 `tar` 在所有目标平台上都存在（Windows 10 1803+ 也自带）。
fn extract_archive(path: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("解析压缩包失败: {e}"))?;
        archive
            .extract(dest)
            .map_err(|e| format!("解压失败: {e}"))?;
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd = std::process::Command::new("tar");
        cmd.args(["-xzf"]).arg(path).arg("-C").arg(dest);

        let out = cmd.output().map_err(|e| format!("调用 tar 失败: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "解压失败（tar 退出码 {:?}）：{}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
fn asset_url_for(source: sources::Source, backend: &str) -> Result<String, String> {
    asset_url_full(source, backend, None)
}

/// 同 [`asset_url_for`]，但允许给 `Source::Custom` 指定镜像基地址。
fn asset_url_full(
    source: sources::Source,
    backend: &str,
    custom_base: Option<&str>,
) -> Result<String, String> {
    let v = PINNED_VERSION;
    let ext = archive_ext();
    let name = if cfg!(target_os = "windows") {
        match backend {
            "cpu" => format!("llama-{v}-bin-win-cpu-x64.{ext}"),
            _ => format!("llama-{v}-bin-win-vulkan-x64.{ext}"),
        }
    } else if cfg!(target_os = "macos") {
        format!("llama-{v}-bin-macos-arm64.{ext}")
    } else {
        match backend {
            "cpu" => format!("llama-{v}-bin-ubuntu-x64.{ext}"),
            _ => format!("llama-{v}-bin-ubuntu-vulkan-x64.{ext}"),
        }
    };

    // URL 走 sources 模块拼，这样"换源"只有一个实现点。
    // 引擎目前只有 GitHub 一个真实来源（上游不在别处发 release），
    // 但把拼接集中到 sources 之后，将来加镜像不用改这里。
    sources::resolve_with(
        source,
        "ggml-org/llama.cpp",
        &format!("{v}/{name}"),
        custom_base,
    )
    .ok_or_else(|| "无法构造引擎下载地址".to_string())
}

fn emit(app: &AppHandle, stage: &str, downloaded: u64, total: u64) {
    let percent = if total > 0 {
        downloaded as f64 / total as f64 * 100.0
    } else {
        0.0
    };
    let _ = app.emit(
        "runtime://progress",
        DownloadProgress {
            stage: stage.to_string(),
            downloaded,
            total,
            percent,
        },
    );
}

fn find_executable(root: &Path) -> Option<PathBuf> {
    let target = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };

    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().and_then(|n| n.to_str()) == Some(target) {
                return Some(path);
            }
        }
    }
    None
}

#[tauri::command]
pub fn runtime_status(app: AppHandle) -> RuntimeStatus {
    status(&app)
}

#[tauri::command]
pub async fn install_runtime(
    app: AppHandle,
    backend: String,
    force: Option<bool>,
) -> Result<RuntimeStatus, String> {
    // 引擎目前只有 GitHub 一个真实来源（上游不在别处发 release），
    // 但走 candidates + download_from_sources 之后，将来加镜像
    // 只需要在 sources.rs 里补一个源，这里零改动。
    let custom = sources::custom_mirror(&app);
    let urls: Vec<String> = sources::candidates_for(&app, sources::Kind::Engine)
        .into_iter()
        .filter_map(|src| asset_url_full(src, &backend, custom.as_deref()).ok())
        .collect();
    if urls.is_empty() {
        return Err("没有可用的引擎下载源".into());
    }
    let root = paths::runtime_root(&app)?;
    let downloads = root.join("downloads");

    /*
     * **已经装过同一个后端就不要重下。**
     *
     * 用户实测：装好引擎之后引导层里那个「下载推理引擎…」按钮还在，
     * 再点一次会重新下 31 MB —— 国内 90 KB/s 要等 5 分钟，白等。
     *
     * 判据是「安装目录里有可执行文件」而不是配置里的 `backend` 字段：
     * 用户手动指定引擎时 `backend` 记作 `manual`，此时 runtime 目录
     * 里并没有我们装的那一份，仍然应该允许下载。所以直接查目录。
     *
     * 换后端（vulkan → cuda）时这一层自然放行：路径按 backend 拼接。
     */
    if let Some(existing) = reinstall_blocked(force.unwrap_or(false), &root, &backend) {
        return Err(format!(
            "这个后端（{backend}）已经装好了：{existing}\n\
             如需重装（例如引擎损坏），请在设置里用「重新下载」。"
        ));
    }

    paths::ensure(&downloads)?;

    let archive_path = downloads.join(format!(
        "llama-{PINNED_VERSION}-{backend}.{}",
        archive_ext()
    ));

    // 下载 + 校验：走通用下载器（断点续传、流式哈希）
    //
    // 早先这里是内联的下载循环，哈希用 `fs::read` 整包读进内存。
    // 31 MB 的运行时包还能接受，但模型动辄几个 GB，那套写法不能复用。
    // 统一走 downloader 之后：续传、限流进度、流式校验都只有一份实现。
    //
    // ## 为什么现在要能取消
    //
    // 早先这里传 `cancel: None`，理由是「31 MB 几秒就完，不值得做取消」。
    // 那个前提是错的：2026-11-14 实测国内加速代理只有 **约 90 KB/s**，
    // 30.8 MB 要下 **5 分钟**。而引导层里用户没有别的出路——要么干等，
    // 要么关掉窗口。关窗口后 `.part` 会留在磁盘上，界面上又看不见。
    //
    // 现在与预设下载共用同一个通道和取消标志（`download_gate`）。
    let gate = crate::download_gate::Gate::acquire()
        .ok_or_else(|| "已有下载在进行，请等它完成或先取消".to_string())?;

    emit(&app, "downloading", 0, 0);
    let app_for_progress = app.clone();
    let result = downloader::download_from_sources(
        &urls,
        &archive_path,
        expected_sha256(&backend),
        Some(gate.flag()),
        |p| {
            emit(&app_for_progress, "downloading", p.downloaded, p.total);
        },
    )
    .await;

    let result = match result {
        Ok(done) => done,
        // 取消不是失败：不发 failed 阶段（界面会弹红字），
        // `install_runtime` 的命令返回值仍是 Err，但文案是中性的。
        Err(downloader::DownloadError::Cancelled) => {
            emit(&app, "cancelled", 0, 0);
            return Err("已取消下载。已下载的部分保留，下次会继续".into());
        }
        Err(e) => return Err(e.to_string()),
    };

    emit(&app, "verifying", 0, 0);
    // 下载器已算过哈希并校验过；这里只留一行日志便于事后核对
    crate::logging::info(
        "runtime",
        format!(
            "引擎包下载完成 {} 字节{}，sha256={}",
            result.bytes,
            if result.resumed { "（续传）" } else { "" },
            result.sha256
        ),
    );

    // 解压
    emit(&app, "extracting", 0, 0);
    let dest = root.join(format!("llama-{PINNED_VERSION}-{backend}"));
    if dest.exists() {
        let _ = std::fs::remove_dir_all(&dest);
    }
    paths::ensure(&dest)?;
    extract_archive(&result.path, &dest)?;

    let exe = find_executable(&dest).ok_or("压缩包内未找到 llama-server")?;

    // 解压完安装包就没用了，31 MB 留着只是白占磁盘（失败路径见下方各 return 分支）
    let _ = std::fs::remove_file(&archive_path);
    // 空掉就顺手把 downloads 目录也收掉；非空时 remove_dir 会失败，正好当"别动"
    let _ = std::fs::remove_dir(&downloads);

    // 自检
    emit(&app, "checking", 0, 0);
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        tokio::process::Command::new(&exe).arg("--version").output(),
    )
    .await
    .map_err(|_| "运行时自检超时（可能是缺少 VC++ 运行库）")?
    .map_err(|e| format!("运行时无法启动: {e}（可能是缺少 VC++ 运行库）"))?;

    if !output.status.success() {
        return Err(format!(
            "运行时自检失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let info = RuntimeInfo {
        version: PINNED_VERSION.to_string(),
        backend: backend.clone(),
        server_path: exe.to_string_lossy().to_string(),
        source: "downloaded".into(),
        sha256: result.sha256.clone(),
    };
    write_info(&app, &info)?;

    emit(&app, "done", 0, 0);
    Ok(status(&app))
}

/// 高级选项：直接指定已有的 llama-server，跳过下载。
///
/// **必须真的能跑起来才算数**：只判断文件存在的话，用户选错一个同名程序
/// （或者选了个损坏的副本）也会被记成"已安装"，直到真正翻译时才炸。
/// 这里先跑一次 `--version`，失败就拒绝并说明原因。
#[tauri::command]
pub fn set_manual_runtime(app: AppHandle, server_path: String) -> Result<RuntimeStatus, String> {
    let path = PathBuf::from(&server_path);
    if !path.exists() {
        return Err(format!("路径不存在：{}", path.display()));
    }
    if !path.is_file() {
        return Err("请选择 llama-server 可执行文件，而不是目录".into());
    }

    let probe = probe_exe(&path);
    if !probe.ok {
        return Err(format!("这个文件不能用作推理引擎：{}", probe.detail));
    }

    let info = RuntimeInfo {
        version: "manual".into(),
        backend: "manual".into(),
        server_path: path.to_string_lossy().to_string(),
        source: "manual".into(),
        sha256: String::new(),
    };
    write_info(&app, &info)?;

    let mut st = status(&app);
    // 把自检结果一起带回去，界面上能直接看到"切换后的引擎到底是哪个版本"
    st.probe = Some(probe);
    Ok(st)
}

/// 磁盘上已有、可直接切换过去的引擎。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredRuntime {
    pub path: String,
    /// 目录名（`llama-b11714-bin-win-cuda-13.4-x64` 这种），便于用户辨认
    pub label: String,
    /// 推断的后端：cuda / vulkan / cpu / unknown
    pub backend: String,
    pub version: String,
}

/// 在常见位置找现成的 llama-server。
///
/// 用户手里往往已经有一份（比如自己下的 CUDA 版），没必要再让人
/// 去文件对话框里翻路径——扫一遍列出来点一下就能换。
#[tauri::command]
pub fn discover_runtimes(app: AppHandle) -> Vec<DiscoveredRuntime> {
    let exe_name = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };

    // 扫描位置：应用自己的 runtime 目录 + 用户常见的下载位置
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(root) = paths::runtime_root(&app) {
        roots.push(root);
    }
    for key in ["USERPROFILE", "HOME"] {
        if let Ok(home) = std::env::var(key) {
            let home = PathBuf::from(home);
            for sub in ["Downloads", "Desktop", "Documents"] {
                let p = home.join(sub);
                if p.is_dir() {
                    roots.push(p);
                }
            }
        }
    }
    // D 盘根目录也常被用来放这类绿色包
    #[cfg(windows)]
    {
        for drive in ["D:\\", "E:\\"] {
            let p = PathBuf::from(drive);
            if p.is_dir() {
                roots.push(p);
            }
        }
    }

    let mut found: Vec<DiscoveredRuntime> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for root in roots {
        // 只看两层：`<root>/llama-xxx/llama-server.exe` 或 `<root>/llama-server.exe`
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&root) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_file() && p.file_name().and_then(|n| n.to_str()) == Some(exe_name) {
                    candidates.push(p.clone());
                }
                if p.is_dir() {
                    let direct = p.join(exe_name);
                    if direct.is_file() {
                        candidates.push(direct);
                    }
                }
            }
        }

        for path in candidates {
            let key = path.to_string_lossy().to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            let label = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("llama.cpp")
                .to_string();
            let lower = label.to_lowercase();
            let backend = if lower.contains("cuda") {
                "cuda"
            } else if lower.contains("vulkan") {
                "vulkan"
            } else if lower.contains("cpu") {
                "cpu"
            } else {
                "unknown"
            }
            .to_string();

            // 版本信息从 --version 拿；拿不到就标成未知，不阻断列表
            let probe = probe_exe(&path);
            found.push(DiscoveredRuntime {
                path: path.to_string_lossy().to_string(),
                label,
                backend,
                version: if probe.ok {
                    probe.detail
                } else {
                    "（无法执行）".into()
                },
            });
        }
    }

    found.sort_by(|a, b| a.label.cmp(&b.label));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 各平台的发布包扩展名必须与 llama.cpp 上游一致。
    ///
    /// **这条是真 bug 的回归点**：之前一律按 `.zip` 拼 URL，
    /// Linux 上直接 404（上游是 `.tar.gz`），用户点「下载引擎」永远失败。
    #[test]
    fn archive_ext_matches_upstream_convention() {
        let ext = archive_ext();
        if cfg!(target_os = "windows") {
            assert_eq!(ext, "zip", "Windows 发布包是 zip");
        } else {
            assert_eq!(ext, "tar.gz", "Linux/macOS 发布包是 tar.gz");
        }
    }

    /// URL 里的扩展名必须与 `archive_ext()` 一致，不能写死。
    #[test]
    fn asset_url_uses_the_right_extension() {
        let url =
            asset_url_for(sources::Source::GitHub, "vulkan").expect("vulkan 后端应当有下载地址");
        let ext = archive_ext();

        assert!(
            url.ends_with(&format!(".{ext}")),
            "URL 应以 .{ext} 结尾，实际是 {url}"
        );
        assert!(
            url.contains(PINNED_VERSION),
            "URL 必须 pin 版本，实际是 {url}"
        );
    }

    /// 后端名要真的体现在文件名里，否则「CPU / Vulkan」会下载同一个包。
    ///
    /// **仅 Windows / Linux**：macOS 上游只发布 `macos-arm64` 一个包
    /// （Metal 后端，不走 Vulkan/CPU 之分），`asset_url` 有意忽略
    /// `backend`。这条断言在 macOS 上不成立，是测试自己的平台假设错了，
    /// 不是实现错了——2026-11-05 在 macOS 上实测发现。
    /// macOS 的对应行为由 `macos_ignores_backend_choice` 覆盖。
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn different_backends_give_different_urls() {
        let a = asset_url_for(sources::Source::GitHub, "vulkan").unwrap();
        let b = asset_url_for(sources::Source::GitHub, "cpu").unwrap();
        assert_ne!(a, b, "不同后端必须对应不同的下载地址");
    }

    /// 每个支持的后端都必须有 SHA-256 校验值。
    ///
    /// 技术债 #7 曾让 Linux/macOS 走「只记录、不阻断」的降级路径——
    /// 等于用户下载的推理运行时**没有被验证过**，在网络层被替换也
    /// 发现不了。现在三个平台的哈希都从 upstream release 页面抄来了，
    /// 这条测试防止它再退化。
    #[test]
    fn every_backend_has_a_sha256() {
        for backend in ["cpu", "vulkan"] {
            let hash = expected_sha256(backend);
            assert!(
                hash.is_some(),
                "{backend} 后端缺少 SHA-256，下载包将无法校验"
            );
            let hash = hash.unwrap();
            assert_eq!(
                hash.len(),
                64,
                "{backend} 的 SHA-256 长度应为 64，实际 {} 位：{hash}",
                hash.len()
            );
            assert!(
                hash.chars().all(|c| c.is_ascii_hexdigit()),
                "{backend} 的 SHA-256 含非十六进制字符：{hash}"
            );
        }
    }

    /// macOS 不分后端：上游只提供 `macos-arm64` 一个包（Metal）。
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_ignores_backend_choice() {
        assert_eq!(
            asset_url_for(sources::Source::GitHub, "cpu").unwrap(),
            asset_url_for(sources::Source::GitHub, "vulkan").unwrap()
        );
    }

    #[test]
    fn missing_path_is_rejected() {
        let probe = probe_exe(Path::new(r"D:\definitely-not-here\llama-server.exe"));
        assert!(!probe.ok);
        assert!(probe.detail.contains("不存在"), "{}", probe.detail);
    }

    /// 目录不能被当成可执行文件——用户很容易选错。
    #[test]
    fn directory_is_rejected() {
        let dir = std::env::temp_dir();
        let probe = probe_exe(&dir);
        assert!(!probe.ok, "目录不该通过自检");
    }

    /// 父目录不存在时 `fs::write` 会报 `os error 3`。
    ///
    /// 锁住一个真实踩过的坑：写清单时目录是懒创建的，首次手动指定引擎时
    /// 它还不存在，`fs::write` 不会替你建父目录，于是装引擎直接失败，
    /// 只报一句没头没尾的「系统找不到指定的路径」。
    ///
    /// 现在引擎信息改存 `config.json`（`set_config_value` 内部会 `ensure`），
    /// 这条仍保留：它验证的是 `ensure` 这个兜底动作本身是有效的。
    #[test]
    fn writing_into_a_missing_dir_needs_ensure() {
        let dir = std::env::temp_dir().join("xieyi-missing-parent-test");
        let _ = std::fs::remove_dir_all(&dir);
        let nested = dir.join("nested").join("probe.json");

        // 先确认现象：不建目录就是 NotFound（Windows 下即 os error 3）
        let err = std::fs::write(&nested, "{}").unwrap_err();
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::NotFound,
            "应当是「找不到路径」而不是别的错：{err}"
        );

        // ensure 之后必须能写进去
        paths::ensure(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, "{}").expect("建好目录后应当能写入");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真实引擎的自检：版本信息在 stderr 里，不是 stdout。
    ///
    /// 这条用 LT_LLAMA_SERVER 指定路径才会跑（CI 上没装引擎）。
    /// 顺带锁住"stdout 为空时改读 stderr"这个容易写错的分支。
    #[test]
    fn real_engine_probe_reads_version_from_stderr() {
        let Ok(exe) = std::env::var("LT_LLAMA_SERVER") else {
            return;
        };
        let probe = probe_exe(Path::new(&exe));
        assert!(probe.ok, "真实引擎应当通过自检：{}", probe.detail);
        assert!(
            probe.detail.to_lowercase().contains("version"),
            "应带回版本行：{}",
            probe.detail
        );
    }

    // ───────────── 重复安装的闸门 ─────────────

    /// `force = true` 必须绕过重复安装的闸门。
    ///
    /// 这条锁住「重新下载」这条修复路径。加了闸门之后，设置里那个
    /// 「重新下载」按钮会直接失败——引擎目录损坏、缺 DLL、启动即崩时，
    /// 用户就没有自救手段了。
    ///
    /// 变异验证：把 `reinstall_blocked` 里的 `if force { return None }`
    /// 删掉，本测试失败。
    #[test]
    fn force_bypasses_the_reinstall_guard() {
        let root = std::env::temp_dir().join("xieyi-force-reinstall-test");
        let _ = std::fs::remove_dir_all(&root);

        let dir = root.join(format!("llama-{PINNED_VERSION}-vulkan"));
        std::fs::create_dir_all(&dir).unwrap();
        let exe_name = if cfg!(windows) {
            "llama-server.exe"
        } else {
            "llama-server"
        };
        std::fs::write(dir.join(exe_name), b"fake").unwrap();

        // 不高抬贵手时：拦住
        assert!(
            reinstall_blocked(false, &root, "vulkan").is_some(),
            "已装且未要求重装时应当拦住"
        );
        // 明确要求重装时：放行
        assert!(
            reinstall_blocked(true, &root, "vulkan").is_none(),
            "force=true 必须放行，否则「重新下载」永远失败，引擎坏了没法自救"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// `force = true` 在没有已安装引擎时也是无害的（设置里首次下载也走它）。
    #[test]
    fn force_is_harmless_when_nothing_is_installed() {
        let root = std::env::temp_dir().join("xieyi-force-empty-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        assert!(reinstall_blocked(false, &root, "vulkan").is_none());
        assert!(reinstall_blocked(true, &root, "vulkan").is_none());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 装过的后端必须被认出来，避免用户重复下载 31 MB。
    ///
    /// 用户实测：引擎装好后引导层的按钮还在，再点一次会重新下一遍。
    /// 国内 90 KB/s 要等 5 分钟，白等。
    ///
    /// 变异验证：把 `already_installed` 改成恒返回 `None`，本测试失败。
    #[test]
    fn installed_backend_is_detected() {
        let root = std::env::temp_dir().join("xieyi-already-installed-test");
        let _ = std::fs::remove_dir_all(&root);

        let dir = root.join(format!("llama-{PINNED_VERSION}-vulkan"));
        std::fs::create_dir_all(&dir).unwrap();

        // 目录建好了但还没有可执行文件 → 不算装好（可能上次解压断了）
        assert!(
            already_installed(&root, "vulkan").is_none(),
            "只有空目录不该算已安装"
        );

        // 放一个同名可执行文件
        let exe_name = if cfg!(windows) {
            "llama-server.exe"
        } else {
            "llama-server"
        };
        std::fs::write(dir.join(exe_name), b"fake").unwrap();

        let found = already_installed(&root, "vulkan").expect("有可执行文件就该算已安装");
        assert!(
            found.contains("vulkan"),
            "返回的路径要能看出是哪个后端：{found}"
        );

        // 换个后端不该误报
        assert!(
            already_installed(&root, "cuda").is_none(),
            "cuda 没装过，不能被 vulkan 的存在误判"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // ───────────── 未完成下载的统计与清理 ─────────────
    //
    // `partial_bytes` / `delete_runtime_partial` 都要 `AppHandle`，
    // 单测里造不出来。所以把"扫目录、认后缀、累加、删除"这段逻辑
    // 抽成纯函数单独测——它才是容易写错的地方（后缀判断、`remove_dir`
    // 失败时的行为、`.meta` 漏删）。
    //
    // 真实的 AppHandle 那层只是"找到目录 → 调纯函数"，没什么可错的。

    use super::{clean_partials_in, sum_partials_in};

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(tag);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 只算 `.part`，不算 `.part.meta`，也不算完整 zip。
    ///
    /// 变异验证：把后缀判断从 `ends_with(".part")` 改成
    /// `contains(".part")`，本测试失败（`.part.meta` 也会被算进去，
    /// 导致统计出来的字节数比实际占用多）。
    #[test]
    fn sum_partials_counts_only_part_files() {
        let dir = tmp_dir("xieyi-runtime-sum-test");
        std::fs::write(dir.join("a.zip.part"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.join("a.zip.part.meta"), b"{\"total\":1000}").unwrap();
        // 完整的 zip 不该被算进来
        std::fs::write(dir.join("b.zip"), vec![0u8; 5000]).unwrap();
        // 别的文件也不该被算
        std::fs::write(dir.join("readme.txt"), vec![0u8; 77]).unwrap();

        assert_eq!(
            sum_partials_in(&dir),
            1000,
            "只该累加 .part 的字节。把 .meta 或完整 zip 算进来会虚报占用"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 目录不存在时返回 0，不 panic。
    #[test]
    fn sum_partials_on_missing_dir_is_zero() {
        let dir = std::env::temp_dir().join("xieyi-runtime-nonexistent-xyz");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(sum_partials_in(&dir), 0);
    }

    /// 清理必须同时删 `.part` 与 `.part.meta`，并报告释放的字节数。
    ///
    /// 变异验证：只删 `.part` 不删 `.meta`，本测试失败——留着 meta 会让
    /// 下次下载按已失效的身份尝试续传，等于白试一次。
    #[test]
    fn clean_partials_removes_both_and_reports_freed() {
        let dir = tmp_dir("xieyi-runtime-clean-test");
        std::fs::write(dir.join("a.zip.part"), vec![0u8; 2048]).unwrap();
        std::fs::write(dir.join("a.zip.part.meta"), b"{}").unwrap();
        // 用户手放的完整包不该被删
        std::fs::write(dir.join("keep.zip"), vec![0u8; 10]).unwrap();

        let freed = clean_partials_in(&dir);
        assert_eq!(freed, 2048, "要报告释放了多少字节，界面才能提示用户");
        assert!(!dir.join("a.zip.part").exists(), ".part 必须删掉");
        assert!(
            !dir.join("a.zip.part.meta").exists(),
            ".part.meta 必须一起删，否则下次续传会按已失效的身份尝试"
        );
        assert!(
            dir.join("keep.zip").exists(),
            "完整的 zip 不该被当成残留删掉（可能是用户手动放的）"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 没有残留时清理是安全的：返回 0，不 panic。
    #[test]
    fn clean_partials_on_empty_dir_is_harmless() {
        let dir = tmp_dir("xieyi-runtime-clean-empty-test");
        assert_eq!(clean_partials_in(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
