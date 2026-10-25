use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

use crate::paths;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelFile {
    pub id: String,
    pub path: String,
    pub size_mb: u64,
}

/// 扫描模型目录下的 GGUF 文件（最多两层子目录）。
pub fn scan(app: &AppHandle) -> Vec<ModelFile> {
    let dir = match paths::models_dir(app) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let _ = paths::ensure(&dir);
    let mut out = Vec::new();
    collect(&dir, 0, &mut out);
    out.sort_by(|a, b| a.id.to_lowercase().cmp(&b.id.to_lowercase()));
    out
}

fn collect(dir: &Path, depth: u8, out: &mut Vec<ModelFile>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < 1 {
                collect(&path, depth + 1, out);
            }
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("gguf") {
            continue;
        }

        // mmproj 是视觉项目的投影文件，不能单独作为模型加载
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.to_lowercase().contains("mmproj") {
            continue;
        }

        let size_mb = std::fs::metadata(&path)
            .map(|m| (m.len() as f64 / 1_048_576.0).round() as u64)
            .unwrap_or(0);

        out.push(ModelFile {
            id: stem(&path),
            path: path.to_string_lossy().to_string(),
            size_mb,
        });
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

/// 生成路由模式用的 preset（每个模型单独限上下文，避免默认 262144 吃爆显存）。
pub fn write_preset(app: &AppHandle, models: &[ModelFile], gpu_layers: u32) -> Result<PathBuf, String> {
    let root = paths::runtime_root(app)?;
    paths::ensure(&root)?;
    let path = root.join("presets.ini");

    let mut ini = String::new();
    for m in models {
        ini.push_str(&format!(
            "[{}]\nmodel = {}\njinja = 1\nctx-size = 2048\nn-gpu-layers = {}\ncache-type-k = q8_0\ncache-type-v = q8_0\n\n",
            m.id, m.path, gpu_layers
        ));
    }

    std::fs::write(&path, ini).map_err(|e| format!("写入 preset 失败: {e}"))?;
    Ok(path)
}
