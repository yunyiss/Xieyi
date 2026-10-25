use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

use crate::paths;

/// 固定的 llama.cpp 构建号。路由模式属于较新特性，必须 pin 版本而不是用 latest。
pub const PINNED_VERSION: &str = "b11714";

/// 已核对过的下载包校验值（按后端区分；没有记录的组合跳过校验、仅保存实际哈希）。
fn expected_sha256(backend: &str) -> Option<&'static str> {
    if cfg!(target_os = "windows") && backend == "vulkan" {
        // llama-b11714-bin-win-vulkan-x64.zip
        Some("d4765447307f29fc0b4c8ae5b8b806c46b3e515d0ce5c2a54dd90d9b2c7a392f")
    } else {
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
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    stage: String,
    downloaded: u64,
    total: u64,
    percent: f64,
}

pub fn read_info(app: &AppHandle) -> Option<RuntimeInfo> {
    let raw = std::fs::read_to_string(paths::runtime_json(app).ok()?).ok()?;
    serde_json::from_str(&raw).ok()
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

fn asset_url(backend: &str) -> Result<String, String> {
    let v = PINNED_VERSION;
    let name = if cfg!(target_os = "windows") {
        match backend {
            "cpu" => format!("llama-{v}-bin-win-cpu-x64.zip"),
            _ => format!("llama-{v}-bin-win-vulkan-x64.zip"),
        }
    } else if cfg!(target_os = "macos") {
        format!("llama-{v}-bin-macos-arm64.zip")
    } else {
        format!("llama-{v}-bin-ubuntu-vulkan-x64.zip")
    };

    Ok(format!(
        "https://github.com/ggml-org/llama.cpp/releases/download/{v}/{name}"
    ))
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
pub async fn install_runtime(app: AppHandle, backend: String) -> Result<RuntimeStatus, String> {
    let url = asset_url(&backend)?;
    let root = paths::runtime_root(&app)?;
    let downloads = root.join("downloads");
    paths::ensure(&downloads)?;

    let zip_path = downloads.join(format!("llama-{PINNED_VERSION}-{backend}.zip"));

    // 下载
    emit(&app, "downloading", 0, 0);
    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("请求下载地址失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "下载失败，HTTP {}。可尝试更换网络或手动放置运行时。",
            resp.status()
        ));
    }
    let total = resp.content_length().unwrap_or(0);

    let mut file = tokio::fs::File::create(&zip_path)
        .await
        .map_err(|e| format!("创建下载文件失败: {e}"))?;
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_emit = std::time::Instant::now();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("下载中断: {e}"))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("写入文件失败: {e}"))?;
        downloaded += chunk.len() as u64;

        if last_emit.elapsed().as_millis() > 250 {
            emit(&app, "downloading", downloaded, total);
            last_emit = std::time::Instant::now();
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    emit(&app, "downloading", downloaded, total);

    // 校验
    emit(&app, "verifying", 0, 0);
    let bytes = std::fs::read(&zip_path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = format!("{:x}", hasher.finalize());
    if let Some(expected) = expected_sha256(&backend) {
        if actual != expected {
            let _ = std::fs::remove_file(&zip_path);
            return Err(format!(
                "下载包校验失败：期望 {expected}，实际 {actual}。文件已删除。"
            ));
        }
    }

    // 解压
    emit(&app, "extracting", 0, 0);
    let dest = root.join(format!("llama-{PINNED_VERSION}-{backend}"));
    if dest.exists() {
        let _ = std::fs::remove_dir_all(&dest);
    }
    paths::ensure(&dest)?;
    let zip_file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(zip_file).map_err(|e| format!("解析压缩包失败: {e}"))?;
    archive
        .extract(&dest)
        .map_err(|e| format!("解压失败: {e}"))?;

    let exe = find_executable(&dest).ok_or("压缩包内未找到 llama-server")?;

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
        sha256: actual,
    };
    let json = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    std::fs::write(paths::runtime_json(&app)?, json).map_err(|e| e.to_string())?;

    emit(&app, "done", 0, 0);
    Ok(status(&app))
}

/// 高级选项：直接指定已有的 llama-server，跳过下载。
#[tauri::command]
pub fn set_manual_runtime(app: AppHandle, server_path: String) -> Result<RuntimeStatus, String> {
    let path = PathBuf::from(&server_path);
    if !path.exists() {
        return Err(format!("路径不存在: {}", path.display()));
    }
    let info = RuntimeInfo {
        version: "manual".into(),
        backend: "manual".into(),
        server_path: path.to_string_lossy().to_string(),
        source: "manual".into(),
        sha256: String::new(),
    };
    let json = serde_json::to_string_pretty(&info).map_err(|e| e.to_string())?;
    std::fs::write(paths::runtime_json(&app)?, json).map_err(|e| e.to_string())?;
    Ok(status(&app))
}
