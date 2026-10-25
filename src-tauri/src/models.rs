use serde::Serialize;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

use crate::hardware;
use crate::paths;

/// 表示"自动按显存分配层数"的哨兵值。
pub const AUTO_LAYERS: u64 = 1001;

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
/// `gpu_layers` 为 [`AUTO_LAYERS`] 时按模型体积和当前空闲显存自动估算。
pub fn write_preset(app: &AppHandle, models: &[ModelFile], gpu_layers: u64) -> Result<PathBuf, String> {
    let root = paths::runtime_root(app)?;
    paths::ensure(&root)?;
    let path = root.join("presets.ini");

    let free_vram = if gpu_layers == AUTO_LAYERS {
        hardware::free_vram_mb()
    } else {
        None
    };

    let mut ini = String::new();
    for m in models {
        let layers = if gpu_layers == AUTO_LAYERS {
            let layer_count = read_layer_count(Path::new(&m.path));
            free_vram
                .map(|free| estimate_layers(m.size_mb, layer_count, free))
                .unwrap_or(999)
        } else {
            gpu_layers
        };

        ini.push_str(&format!(
            "[{}]\nmodel = {}\njinja = 1\nctx-size = 4096\nn-gpu-layers = {}\ncache-type-k = q8_0\ncache-type-v = q8_0\n\n",
            m.id, m.path, layers
        ));
    }

    std::fs::write(&path, ini).map_err(|e| format!("写入 preset 失败: {e}"))?;
    Ok(path)
}

/// 估算能放进显存的层数。
///
/// `size_mb` 是量化后的权重体积，`layers` 缺省按 32 层算。
/// 预留：桌面/其他程序约 600MB、KV cache 与计算缓冲约体积的 4% + 200MB。
fn estimate_layers(size_mb: u64, layers: Option<u32>, free_mb: u64) -> u64 {
    let layers = layers.unwrap_or(32).max(1) as f64;
    let weights = size_mb as f64 * 1.06;
    let overhead = size_mb as f64 * 0.04 + 800.0;
    let budget = free_mb as f64 - overhead;

    if budget >= weights {
        return 999;
    }
    if budget <= 0.0 {
        return 0;
    }

    ((layers * (budget / weights)).floor() as u64).min(layers as u64)
}

/// 从 GGUF 头部读取 `*.block_count`，用于按比例分配 GPU 层数。
pub fn read_layer_count(path: &Path) -> Option<u32> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).ok()?;
    if &magic != b"GGUF" {
        return None;
    }

    let version = read_u32(&mut reader)?;
    if version < 2 {
        return None;
    }

    let _tensor_count = read_u64(&mut reader)?;
    let kv_count = read_u64(&mut reader)?;

    for _ in 0..kv_count {
        let key = read_string(&mut reader)?;
        let vtype = read_u32(&mut reader)?;

        if key.ends_with("block_count") {
            match vtype {
                4 => return read_u32(&mut reader),
                10 => return read_u64(&mut reader).map(|v| v as u32),
                _ => {
                    skip_value(&mut reader, vtype, 0)?;
                    continue;
                }
            }
        }

        skip_value(&mut reader, vtype, 0)?;
    }

    None
}

fn read_u8<R: Read>(r: &mut R) -> Option<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b).ok()?;
    Some(b[0])
}

fn read_u32<R: Read>(r: &mut R) -> Option<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).ok()?;
    Some(u32::from_le_bytes(b))
}

fn read_u64<R: Read>(r: &mut R) -> Option<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b).ok()?;
    Some(u64::from_le_bytes(b))
}

fn read_string<R: Read>(r: &mut R) -> Option<String> {
    let len = read_u64(r)? as usize;
    if len > 4096 {
        return None;
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}

/// 跳过任意类型的元数据值。
fn skip_value<R: Read>(r: &mut R, vtype: u32, depth: u8) -> Option<()> {
    if depth > 8 {
        return None;
    }
    match vtype {
        0 | 1 | 7 => {
            read_u8(r)?;
        }
        2 | 3 => {
            let mut b = [0u8; 2];
            r.read_exact(&mut b).ok()?;
        }
        4 | 5 | 6 => {
            let mut b = [0u8; 4];
            r.read_exact(&mut b).ok()?;
        }
        8 => {
            read_string(r)?;
        }
        9 => {
            let elem_type = read_u32(r)?;
            let count = read_u64(r)?;
            if count > 100_000 {
                return None;
            }
            for _ in 0..count {
                skip_value(r, elem_type, depth + 1)?;
            }
        }
        10 | 11 | 12 => {
            let mut b = [0u8; 8];
            r.read_exact(&mut b).ok()?;
        }
        _ => return None,
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_model_fits_fully_on_gpu() {
        // 1.1GB 模型，8GB 空闲：应全量上 GPU
        assert_eq!(estimate_layers(1100, Some(28), 7188), 999);
    }

    #[test]
    fn large_model_gets_partial_layers() {
        // 5.5GB 模型只有 3GB 空闲：应只放一部分层
        let layers = estimate_layers(5560, Some(40), 3000);
        assert!(layers > 0 && layers < 40, "got {layers}");
    }

    #[test]
    fn no_free_vram_falls_back_to_cpu() {
        assert_eq!(estimate_layers(5560, Some(40), 200), 0);
    }

    /// 用真实模型校验 GGUF 头部解析；未提供样例文件时自动跳过。
    #[test]
    fn parses_real_gguf_when_provided() {
        let Ok(sample) = std::env::var("LT_GGUF_SAMPLE") else {
            return;
        };
        let layers = read_layer_count(Path::new(&sample));
        assert!(
            layers.map(|n| n > 0).unwrap_or(false),
            "未能从 {sample} 解析出 block_count"
        );
    }
}
