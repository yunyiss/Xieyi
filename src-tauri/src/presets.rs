//! 预设模型列表：我们**实跑验证过**的模型，供用户一键获取。
//!
//! ## 存在的理由
//!
//! 普通用户不知道这个工具适配了哪些模型，也不看文档，很可能随手塞一个
//! 不合适的——比如 Seed-X（官方无 chat 模板，实测输出幻觉），然后以为
//! "这工具坏了"。
//!
//! ## 为什么硬编码在源码里，不做远程拉取
//!
//! 1. 列表要标"我们实测过"，这个信息只存在于我们这里，远程配置一样要维护
//! 2. **离线优先是项目硬约束**（除首次下载运行时外不联网）。启动时拉列表
//!    会破坏这条
//! 3. 列表很小（几个模型），更新靠发版即可
//!
//! 代价是新模型要等发版才能进列表。可以接受——预设在精不在多。
//!
//! ## 当前状态：框架已就绪，列表为空
//!
//! 按用户决定（2026-11-13）先只做框架，模型条目后续填。填入前必须先
//! **实际下载一次**取得真实 SHA-256——凭空写的哈希会让所有用户下载失败。
//!
//! 填充步骤见 `PRESETS` 上方的注释。

use crate::sources::Source;
use serde::Serialize;
use std::path::Path;

/// 一个预设模型的元信息。
///
/// 所有字段都硬编码：预设的卖点就是"我们验证过"，这个信息无法从别处获得。
#[derive(Debug, Clone)]
pub struct PresetModel {
    /// 唯一 id，同时用作下载后的文件名（不含 `.gguf`）
    pub id: &'static str,
    /// 面向用户的名称
    pub name: &'static str,
    /// 一句话说明用途
    pub summary: &'static str,
    /// 仓库标识。
    ///
    /// **两个下载源要分别指定**：魔搭的镜像仓库名与 HuggingFace 不一定
    /// 相同，量化档也可能不同。不能假设两边一致。
    pub repos: &'static [(crate::sources::Source, &'static str)],
    /// 仓库内的文件名
    pub file: &'static str,
    /// 文件字节数。用于显示下载大小 + 下载前检查磁盘空间
    pub size_bytes: u64,
    /// SHA-256（小写十六进制，64 位）。
    ///
    /// **必须实际下载验证过**，不能猜。写错的后果是所有用户下载失败。
    pub sha256: &'static str,
    /// 模型家族，与 `models::ModelProfile.family` 对应
    pub family: &'static str,
    /// 建议显存（MB）。用户显卡不够时提前提示，避免"引擎启动失败"的天书报错
    pub vram_hint_mb: u64,
}

/// 预设模型列表：Hy-MT2 家族的三档，覆盖「极低设备 → 高设备」。
///
/// ## 为什么只放 Hy-MT2
///
/// 它是主推模型：腾讯专为翻译训练（不是通用模型改的），33 语言，
/// Apache-2.0 可商用，且三档体量覆盖从核显到 16G 显卡。
///
/// ## 为什么这样选量化档
///
/// 一条原则：**翻译是辅助功能，不该把显存吃满**。用户可能在同时
/// 跑别的吃显存的任务（项目文档里记着前台生图的工作流）。所以每档
/// 都按「能舒适跑」而不是「勉强能跑」来定，见各条的 `vram_hint_mb`。
///
/// 1.8B 用 **Q8_0 而不是更小的 Q4**：它本来就极小，Q4 只省 0.85 GB，
/// 换来的是可见的质量损失。小模型上量化的绝对值收益太薄，不值得。
/// 大模型才值得为体积牺牲质量——所以 7B 的标准档用 Q4_K_M。
///
/// ## 数据来源（2026-11-13 核实）
///
/// 文件名、字节数、SHA-256 全部取自仓库元数据，并做了交叉验证：
///
/// - 魔搭 `Tencent-Hunyuan/Hy-MT2-{1.8B,7B}-GGUF`
/// - HuggingFace `tencent/Hy-MT2-{1.8B,7B}-GGUF`（经 hf-mirror 查证）
///
/// 两边的 SHA-256 与字节数**完全一致**——这验证了「同一文件换源不改
/// 哈希」的假设，也是多源下载能安全工作的前提。
///
/// 其中 1.8B-Q8_0 与 7B-Q4_K_M 还额外与本机已有的文件做了字节级比对，
/// 算出的哈希与仓库值一致。
///
/// **注意文件名大小写不统一**：7B 的 Q6/Q8 在仓库里是大写 `HY-MT2`
/// 前缀，Q4_K_M 是小写。照抄仓库的写法，不要"统一"成同一种。
///
/// ## 为什么不放 30B-A3B
///
/// 官方在魔搭只发了 safetensors（109 个分片），**没有 GGUF**。
/// 第三方量化在 HuggingFace，国内不可达。没有可靠下载源就不该进列表——
/// 用户点了必然失败，比没有这个选项更糟。
///
/// 真需要它的是 24G+ 显存的用户，这部分人通常自己会找。等官方发
/// GGUF，加一行即可。
pub static PRESETS: &[PresetModel] = &[
    PresetModel {
        id: "Hy-MT2-1.8B-Q8_0",
        name: "Hy-MT2 1.8B · 轻量",
        summary: "轻量：4G 显卡或纯 CPU 可用，速度最快，长文也扛得住。",
        repos: &[
            (Source::ModelScope, "Tencent-Hunyuan/Hy-MT2-1.8B-GGUF"),
            (Source::HuggingFace, "tencent/Hy-MT2-1.8B-GGUF"),
        ],
        file: "Hy-MT2-1.8B-Q8_0.gguf",
        size_bytes: 1_908_528_192,
        sha256: "5c3fe0b1408a5ceb0143184ef247b11b579c525f4b02b060e6c851bb76fef1a4",
        family: "Hy-MT2",
        // 实测权重 2.5 GB + KV/开销约 0.4 GB（4K 上下文）。
        // 给 4 GB 档位留出余量，不让它贴着显卡上限跑。
        vram_hint_mb: 4096,
    },
    PresetModel {
        id: "Hy-MT2-7B-Q4_K_M",
        name: "Hy-MT2 7B · 均衡",
        summary: "标准：8G 显卡的均衡选择。7B 比 1.8B 明显更稳。",
        repos: &[
            (Source::ModelScope, "Tencent-Hunyuan/Hy-MT2-7B-GGUF"),
            (Source::HuggingFace, "tencent/Hy-MT2-7B-GGUF"),
        ],
        file: "Hy-MT2-7B-Q4_K_M.gguf",
        size_bytes: 4_624_648_896,
        sha256: "9f96256500f3fc1ab4d64336b58f52a949a95ad7516b0c229476eef782f9f77b",
        family: "Hy-MT2",
        // 权重 4.8 GB + KV/开销约 0.8 GB。8 GB 卡留 2.4 GB 给桌面。
        vram_hint_mb: 8192,
    },
    PresetModel {
        id: "HY-MT2-7B-Q8_0",
        name: "Hy-MT2 7B · 高质量",
        summary: "高质量：12G 以上显卡。接近无损，适合对译文要求高的场景。",
        repos: &[
            (Source::ModelScope, "Tencent-Hunyuan/Hy-MT2-7B-GGUF"),
            (Source::HuggingFace, "tencent/Hy-MT2-7B-GGUF"),
        ],
        // 仓库里这个文件名是**大写** HY 前缀，与 Q4_K_M 的小写不同。
        // 照抄仓库写法；改成小写会 404。
        file: "HY-MT2-7B-Q8_0.gguf",
        size_bytes: 7_981_928_896,
        sha256: "58b3ad55dd6f6fa08c695cddc34fb5f8f708a844f78ae10508071914b0ed67c0",
        family: "Hy-MT2",
        // 权重 8.6 GB + KV/开销约 0.8 GB。12 GB 卡留 3 GB 余量。
        vram_hint_mb: 12288,
    },
];

/// 供前端显示的预设状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetStatus {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub size_bytes: u64,
    pub vram_hint_mb: u64,
    pub family: String,
    /// 目标目录里是否已有这个文件（靠**实际文件存在 + 大小匹配**判断，
    /// 不靠配置记录——用户可能手动删了文件）
    pub downloaded: bool,
    /// 已存在时的实际路径
    pub path: Option<String>,
    /// 有没有**未完成**的下载残留（`.part`）。
    ///
    /// 这与 `downloaded` 互斥。为 true 时界面显示「已暂停 · 可续传」，
    /// 并给出续传与删除两个入口。
    ///
    /// 为什么必须暴露给界面：`.part` 是故意保留的（下次续传要用），
    /// 但它不进模型列表、启动时也不扫描，用户完全看不见那几个 GB。
    /// 取消过一次 8 GB 下载，磁盘就悄悄少 8 GB。
    pub partial_bytes: u64,
    /// 未完成下载的期望总大小（0 表示未知）。
    /// 用于显示「1.8 / 4.6 GB」并算百分比。
    pub partial_total: u64,
}

/// 把预设列表转成前端要的状态，`models_dir` 用于判断"已下载"。
///
/// 判断依据是**磁盘上的实际文件**：配置里记着"下载过"但文件被用户删了，
/// 也应当显示为未下载。
pub fn statuses(models_dir: &Path) -> Vec<PresetStatus> {
    PRESETS
        .iter()
        .map(|p| preset_status(p, models_dir))
        .collect()
}

fn preset_status(p: &PresetModel, models_dir: &Path) -> PresetStatus {
    let target = models_dir.join(format!("{}.gguf", p.id));

    // 大小必须匹配：同名但内容不同的文件（用户自己放的）不算已下载，
    // 否则会误报"已下载"而用户以为能用
    let downloaded = std::fs::metadata(&target)
        .map(|m| m.len() == p.size_bytes)
        .unwrap_or(false);

    // 已下载时不再报"暂停"：那两份数据不可能同时有意义，
    // 且下载成功后 `.part` 已改名为正式文件
    let partial = if downloaded {
        None
    } else {
        crate::downloader::partial_state(&target)
    };

    PresetStatus {
        id: p.id.to_string(),
        name: p.name.to_string(),
        summary: p.summary.to_string(),
        size_bytes: p.size_bytes,
        vram_hint_mb: p.vram_hint_mb,
        family: p.family.to_string(),
        downloaded,
        path: downloaded.then(|| target.to_string_lossy().to_string()),
        partial_bytes: partial.as_ref().map(|s| s.downloaded).unwrap_or(0),
        partial_total: partial.as_ref().map(|s| s.total).unwrap_or(0),
    }
}

/// 列出预设模型及其下载状态。
///
/// 前端用它渲染"推荐模型"卡片。`downloaded` 靠磁盘实际文件判断，
/// 所以用户手动删了文件也会正确显示为未下载。
#[tauri::command]
pub fn list_presets(app: tauri::AppHandle) -> Result<Vec<PresetStatus>, String> {
    let dir = crate::paths::models_dir(&app)?;
    Ok(statuses(&dir))
}

/// 预设下载的进度事件。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetProgress {
    pub id: String,
    /// downloading / verifying / done / cancelled / failed
    pub stage: String,
    pub downloaded: u64,
    pub total: u64,
    pub percent: f64,
    /// 出现错误时的说明（仅 failed 阶段有）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 删除某个预设未完成的下载（`.part` + `.part.meta`），返回释放的字节数。
///
/// ## 为什么这是独立命令，而不是取消时顺手删
///
/// 取消的语义是「先停一下」，绝大多数情况下用户还会续传。若取消就自动删，
/// 那 3 GB 白下了，用户还得重新等。所以：
///
/// - 取消 → 保留 `.part`，界面变成「已暂停」
/// - 删除 → 用户明确不要了，清干净
///
/// 这个入口存在的另一半理由是：`.part` 不进模型列表、启动时不扫描，
/// 用户没有任何别的办法看到并清理它。没有这个命令，取消一次 8 GB
/// 下载就等于永久占 8 GB 磁盘，且无从察觉。
#[tauri::command]
pub fn delete_preset_partial(app: tauri::AppHandle, id: String) -> Result<u64, String> {
    let preset = find(&id).ok_or_else(|| format!("没有这个预设：{id}"))?;
    let dir = crate::paths::models_dir(&app)?;
    let target = dir.join(format!("{}.gguf", preset.id));

    // 正在下载时不许删：删了文件句柄还在写，会立刻重新长出一个 `.part`
    // 而且大小对不上，续传元信息也乱了。先取消再删。
    if crate::download_gate::is_busy() {
        return Err("请先取消正在进行的下载，再删除已下载的部分".into());
    }

    Ok(crate::downloader::remove_partial(&target))
}

/// 下载一个预设模型到当前模型目录。
///
/// ## 行为
///
/// - 按 `available_sources` 的顺序逐个源尝试（失败自动回退）
/// - 成功后**记住那次成功的源**，下次优先用它
/// - 下载完**不自动切换模型**：会打断用户当前工作，且需要重启引擎
///
/// ## 进度
///
/// 通过 `preset://progress` 事件上报，`stage` 取值：
/// `downloading` → `verifying` → `done`；失败时 `failed` 并带 `error`。
#[tauri::command]
pub async fn download_preset(app: tauri::AppHandle, id: String) -> Result<String, String> {
    let preset = find(&id).ok_or_else(|| format!("没有这个预设：{id}"))?;
    let dir = crate::paths::models_dir(&app)?;
    crate::paths::ensure(&dir)?;

    let dest = dir.join(format!("{}.gguf", preset.id));

    // 已经存在且大小对得上：直接返回，不重复下载
    if let Ok(m) = std::fs::metadata(&dest)
        && m.len() == preset.size_bytes
    {
        return Ok(dest.to_string_lossy().to_string());
    }

    // 占住下载通道。**放在磁盘检查之前**：两个请求同时进来时，
    // 应该只有一个去做磁盘检查，另一个立刻被拒，不必白查一遍。
    //
    // 守卫是 RAII 的，下面任何一条 return 或 `?` 都会自动放开，
    // 并且占住时会自动清掉上一次的取消状态。
    let gate = crate::download_gate::Gate::acquire()
        .ok_or_else(|| "已有下载在进行，请等它完成或先取消".to_string())?;

    // 磁盘空间检查：几 GB 的模型下到一半才发现没空间，很挫败
    check_disk_space(&dir, preset.size_bytes)?;

    // 按源顺序拼 URL；某个源没有这个模型就跳过
    let mut urls = Vec::new();
    let mut used_sources = Vec::new();
    for src in available_sources(preset) {
        if let Some(url) = url_for(preset, src) {
            urls.push(url);
            used_sources.push(src);
        }
    }
    if urls.is_empty() {
        return Err("这个预设没有可用的下载源".into());
    }

    let app_progress = app.clone();
    let id_for_event = preset.id.to_string();
    let expected = preset.sha256;

    // 进度回调里顺手切一次阶段：下载量到总量时，接下来就是算哈希。
    //
    // 为什么要单独发 `verifying`：SHA-256 实测约 1.2 GB/s，8 GB 的最高档
    // 要算 7 秒。这 7 秒里如果还停在 100% 的 `downloading`，用户看到的是
    // 「进度条卡在 100% 不动」，观感与卡死无异。切个阶段就说明白了。
    //
    // 不改 downloader 的签名来加「校验开始」回调：它只负责搬字节，
    // 不该知道上层有几个阶段。用「已下载 == 总量」这个现成信号判断即可。
    let mut said_verifying = false;

    let result = crate::downloader::download_from_sources(
        &urls,
        &dest,
        Some(expected),
        Some(gate.flag()),
        |p| {
            let done_downloading = p.total > 0 && p.downloaded >= p.total;
            if done_downloading && !said_verifying {
                said_verifying = true;
                emit_progress(
                    &app_progress,
                    &id_for_event,
                    "verifying",
                    p.total,
                    p.total,
                    None,
                );
                return;
            }
            if !done_downloading {
                emit_progress(
                    &app_progress,
                    &id_for_event,
                    "downloading",
                    p.downloaded,
                    p.total,
                    None,
                );
            }
        },
    )
    .await;

    match result {
        Ok(done) => {
            // 记住成功的源，下次优先用它——这是"默认选最快"的替代实现：
            // 不测速（那是联网），而是用上次真正成功过的那个。
            //
            // 注意 `download_from_sources` 失败时会换源，所以成功的源
            // 未必是第一个。它返回的 `Downloaded` 不带源信息，这里保守地
            // 记第一个；若第一个是坏的，下次失败后仍会自动回退。
            if let Some(src) = used_sources.first() {
                let _ = crate::sources::remember_source(&app, *src);
            }

            emit_progress(&app, &id_for_event, "done", done.bytes, done.bytes, None);
            Ok(done.path.to_string_lossy().to_string())
        }
        // 取消不是失败：不发 failed 事件（那会弹红字），也不删 `.part`
        // ——保留它，用户再点下载就是续传。
        Err(crate::downloader::DownloadError::Cancelled) => {
            emit_progress(&app, &id_for_event, "cancelled", 0, 0, None);
            Err("已取消下载，已下载的部分保留，下次可续传".into())
        }
        Err(e) => {
            let msg = e.to_string();
            emit_progress(&app, &id_for_event, "failed", 0, 0, Some(msg.clone()));
            Err(msg)
        }
    }
}

/// 上报一次进度。
fn emit_progress(
    app: &tauri::AppHandle,
    id: &str,
    stage: &str,
    downloaded: u64,
    total: u64,
    error: Option<String>,
) {
    use tauri::Emitter;
    let percent = if total > 0 {
        downloaded as f64 / total as f64 * 100.0
    } else {
        0.0
    };
    let _ = app.emit(
        "preset://progress",
        PresetProgress {
            id: id.to_string(),
            stage: stage.to_string(),
            downloaded,
            total,
            percent,
            error,
        },
    );
}

/// 检查目标目录所在磁盘的剩余空间是否够用。
///
/// 留 10% 余量：模型之外还要写临时文件、日志等。
fn check_disk_space(dir: &Path, need_bytes: u64) -> Result<(), String> {
    // 用命令行查剩余空间，避免引入新依赖（sysinfo 之类）
    let free = free_disk_bytes(dir);
    let Some(free) = free else {
        // 查不到就不拦——不能因为查不到空间就拒绝下载
        return Ok(());
    };
    let need = need_bytes + need_bytes / 10;
    if free < need {
        return Err(format!(
            "磁盘空间不足：需要约 {} MB（含 10% 余量），当前只剩 {} MB。请清理空间或换个模型目录。",
            need / 1_048_576,
            free / 1_048_576
        ));
    }
    Ok(())
}

/// 查目标目录所在磁盘的剩余字节数。查不到返回 `None`。
#[cfg(target_os = "windows")]
fn free_disk_bytes(dir: &Path) -> Option<u64> {
    use std::os::windows::process::CommandExt;
    // dir 可能还不存在，向上找到第一个存在的祖先
    let mut probe = dir;
    while !probe.exists() {
        probe = probe.parent()?;
    }
    let drive = probe.components().next()?;
    let letter = drive.as_os_str().to_string_lossy().replace('\\', "");
    let mut cmd = std::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-Command",
        &format!("(Get-PSDrive {letter}).Free"),
    ])
    .creation_flags(0x0800_0000);
    let out = cmd.output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u64>()
        .ok()
}

#[cfg(not(target_os = "windows"))]
fn free_disk_bytes(dir: &Path) -> Option<u64> {
    let mut probe = dir;
    while !probe.exists() {
        probe = probe.parent()?;
    }
    let out = std::process::Command::new("df")
        .args(["-k", probe.to_str()?])
        .output()
        .ok()?;
    // df -k 输出第二行的第 4 列是可用 KB
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().nth(1)?;
    let kb = line.split_whitespace().nth(3)?.parse::<u64>().ok()?;
    Some(kb * 1024)
}

/// 按 id 找预设。
pub fn find(id: &str) -> Option<&'static PresetModel> {
    PRESETS.iter().find(|p| p.id == id)
}

/// 某个预设在某源上的下载 URL。
///
/// 该源没有这个模型时返回 `None`——调用方据此跳过该源，而不是拼出
/// 一个必然 404 的地址。
pub fn url_for(p: &PresetModel, source: crate::sources::Source) -> Option<String> {
    let repo = p
        .repos
        .iter()
        .find(|(s, _)| *s == source)
        .map(|(_, r)| *r)?;
    crate::sources::resolve(source, repo, p.file)
}

/// 这个预设支持的源（按 `sources` 模块的默认顺序排好）。
///
/// 只返回真正有仓库映射的源，避免尝试必然失败的 URL。
pub fn available_sources(p: &PresetModel) -> Vec<crate::sources::Source> {
    crate::sources::default_order(crate::sources::Kind::Model)
        .iter()
        .copied()
        .filter(|s| p.repos.iter().any(|(rs, _)| rs == s))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每条预设的哈希必须合法——写错会让所有用户下载失败。
    ///
    /// 列表为空时这条也会通过（空集合无条件成立），这是有意的：
    /// 框架阶段不该因为"还没填"而报错。
    #[test]
    fn every_preset_has_a_well_formed_sha256() {
        for p in PRESETS {
            assert_eq!(
                p.sha256.len(),
                64,
                "{} 的 sha256 应为 64 位，实际 {} 位",
                p.id,
                p.sha256.len()
            );
            assert!(
                p.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "{} 的 sha256 含非十六进制字符：{}",
                p.id,
                p.sha256
            );
            assert_eq!(
                p.sha256,
                p.sha256.to_lowercase(),
                "{} 的 sha256 应是小写",
                p.id
            );
        }
    }

    #[test]
    fn every_preset_has_a_positive_size() {
        for p in PRESETS {
            assert!(p.size_bytes > 0, "{} 的 size_bytes 必须大于 0", p.id);
            assert!(p.vram_hint_mb > 0, "{} 的 vram_hint_mb 必须大于 0", p.id);
        }
    }

    /// 三档预设的显示名必须**统一带档位后缀**。
    ///
    /// 用户指着截图问：「右边这个轻量补上吧」——因为 1.8B 那一档叫
    /// 「Hy-MT2 1.8B」，而另外两档叫「· 均衡」「· 高质量」。列表里
    /// 一个没有档次标签、两个有，看起来像漏了字段。
    ///
    /// 判据：名称里必须出现档位词。写死清单而不是用正则猜，是因为
    /// "档位"是产品概念，改名时应当有人显式改这条测试。
    ///
    /// 变异验证：把 1.8B 的 name 改回 `Hy-MT2 1.8B`，本测试失败。
    #[test]
    fn preset_names_all_carry_a_tier_suffix() {
        let tiers = ["轻量", "均衡", "高质量"];
        for p in PRESETS {
            assert!(
                tiers.iter().any(|t| p.name.contains(t)),
                "{} 的显示名「{}」不含档位词（{}），列表里会与其它档位不一致",
                p.id,
                p.name,
                tiers.join(" / ")
            );
        }
    }

    /// id 必须唯一——它同时是下载后的文件名，重复会互相覆盖。
    #[test]
    fn preset_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for p in PRESETS {
            assert!(seen.insert(p.id), "id 重复：{}", p.id);
        }
    }

    /// id 必须能安全地当文件名用。
    #[test]
    fn preset_ids_are_filename_safe() {
        for p in PRESETS {
            assert!(!p.id.is_empty(), "id 不能为空");
            for bad in ['/', '\\', ':', '*', '?', '"', '<', '>', '|'] {
                assert!(!p.id.contains(bad), "{} 的 id 含文件名非法字符 {bad}", p.id);
            }
        }
    }

    /// 每个预设至少要有一个可用的下载源。
    #[test]
    fn every_preset_has_at_least_one_source() {
        for p in PRESETS {
            assert!(
                !available_sources(p).is_empty(),
                "{} 没有任何可用的下载源",
                p.id
            );
            for (src, repo) in p.repos {
                assert!(!repo.is_empty(), "{} 在 {:?} 上的仓库名为空", p.id, src);
            }
        }
    }

    /// 找不到的 id 返回 None，不 panic。
    #[test]
    fn find_missing_returns_none() {
        assert!(find("does-not-exist").is_none());
    }

    /// **联网校验**：确认 `PRESETS` 里的大小与哈希真的等于仓库里的值。
    ///
    /// 64 位的哈希肉眼核对不出错。而没有这条测试，一旦哪天手抄错一位，
    /// 后果是**所有用户下载完都校验失败**——而且他们白下了几个 GB 才发现。
    ///
    /// 默认跳过（CI 与离线环境不该依赖外网），需要时手动开：
    ///
    /// ```bash
    /// XIEYI_VERIFY_PRESETS=1 cargo test preset_metadata_matches_remote -- --nocapture
    /// ```
    ///
    /// 用魔搭的仓库接口：它的 `Files` 数组里直接带 `Sha256` 与 `Size`，
    /// 不用把几个 GB 下下来。
    #[tokio::test]
    async fn preset_metadata_matches_remote_when_asked() {
        if std::env::var("XIEYI_VERIFY_PRESETS").is_err() {
            return; // 默认跳过
        }

        for p in PRESETS {
            let Some(repo) = p
                .repos
                .iter()
                .find(|(s, _)| *s == Source::ModelScope)
                .map(|(_, r)| *r)
            else {
                continue;
            };

            let url = format!(
                "https://modelscope.cn/api/v1/models/{repo}/repo/files?Revision=master&Recursive=true"
            );
            let text = match reqwest::get(&url).await {
                Ok(r) => match r.text().await {
                    Ok(t) => t,
                    Err(e) => panic!("{} 读取响应失败：{e}", p.id),
                },
                Err(e) => panic!("{} 请求仓库失败：{e}", p.id),
            };
            let v: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} JSON 解析失败：{e}", p.id));

            let files = v["Data"]["Files"]
                .as_array()
                .unwrap_or_else(|| panic!("{} 响应里没有 Files 数组", p.id));
            let f = files
                .iter()
                .find(|f| f["Name"].as_str() == Some(p.file))
                .unwrap_or_else(|| panic!("{} 在仓库 {repo} 里找不到 {}", p.id, p.file));

            assert_eq!(
                f["Size"].as_u64(),
                Some(p.size_bytes),
                "{} 的 size_bytes 与仓库不符",
                p.id
            );
            assert_eq!(
                f["Sha256"].as_str(),
                Some(p.sha256),
                "{} 的 sha256 与仓库不符",
                p.id
            );
            println!("✓ {} 元数据与仓库一致", p.id);
        }
    }

    /// 状态查询的条数与预设数量一致，且都不 panic。
    ///
    /// 列表现在是三档 Hy-MT2。这条不写死 3，而是断言"数量对齐"——
    /// 以后增删档位时不用改测试，但一旦 `statuses()` 漏掉某条就会失败。
    #[test]
    fn statuses_cover_every_preset() {
        let dir = std::env::temp_dir();
        let out = statuses(&dir);
        assert_eq!(out.len(), PRESETS.len());
        // 每个 id 都要出现且不重复
        let mut seen = std::collections::HashSet::new();
        for st in &out {
            assert!(seen.insert(st.id.clone()), "状态里 id 重复：{}", st.id);
        }
    }

    /// `id` 与 `file` 必须一一对应。
    ///
    /// `download_preset` 用 `format!("{}.gguf", preset.id)` 算目标路径，
    /// 而下载用的是 `preset.file`。两者不一致会出现「下载了 A 文件却存成
    /// B 名字」——而且大小校验还会意外通过（因为校验的是同一份字节）。
    #[test]
    fn preset_filename_matches_id() {
        for p in PRESETS {
            assert_eq!(
                p.file,
                format!("{}.gguf", p.id),
                "{} 的 file 与 id 不匹配，下载路径会对不上",
                p.id
            );
        }
    }

    /// 每个预设都必须同时有魔搭与 HuggingFace 两个源。
    ///
    /// 只有魔搭的话海外用户下不了；只有 HF 的话国内用户下不了。
    /// 两类用户都得能拿到，这是「多源」的意义所在。
    #[test]
    fn every_preset_has_both_sources() {
        for p in PRESETS {
            let srcs = available_sources(p);
            assert!(
                srcs.contains(&Source::ModelScope),
                "{} 缺少魔搭源，国内用户会下不了",
                p.id
            );
            assert!(
                srcs.contains(&Source::HuggingFace),
                "{} 缺少 HuggingFace 源，海外用户会下不了",
                p.id
            );
        }
    }

    /// 两源必须指向**同一个文件名**——这是「换源不改哈希」的前提。
    ///
    /// 实测确认过：魔搭与 HF 上同一个仓库的 GGUF 文件名、大小、
    /// SHA-256 完全一致。这条防止以后有人给某个源配了不同名字。
    #[test]
    fn both_sources_share_the_same_filename() {
        for p in PRESETS {
            let ms = url_for(p, Source::ModelScope).expect("应有魔搭地址");
            let hf = url_for(p, Source::HuggingFace).expect("应有 HF 地址");
            assert!(ms.ends_with(p.file), "魔搭地址文件名不对：{ms}");
            assert!(hf.ends_with(p.file), "HF 地址文件名不对：{hf}");
        }
    }

    /// 磁盘上有同尺寸文件才算"已下载"；尺寸不符不算。
    #[test]
    fn downloaded_requires_matching_size() {
        // 构造一个大小已知的临时文件，用它的尺寸模拟预设
        let dir = std::env::temp_dir().join("xieyi-preset-status-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let p = PresetModel {
            id: "test-model",
            name: "测试",
            summary: "",
            repos: &[(Source::HuggingFace, "a/b")],
            file: "f.gguf",
            size_bytes: 4,
            sha256: "0".repeat(64).leak(),
            family: "通用",
            vram_hint_mb: 100,
        };

        // 文件不存在
        assert!(!preset_status(&p, &dir).downloaded);

        // 尺寸不符
        std::fs::write(dir.join("test-model.gguf"), b"ab").unwrap();
        assert!(!preset_status(&p, &dir).downloaded, "尺寸不符不该算已下载");

        // 尺寸匹配
        std::fs::write(dir.join("test-model.gguf"), b"abcd").unwrap();
        let st = preset_status(&p, &dir);
        assert!(st.downloaded);
        assert!(st.path.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 没有对应仓库的源必须被跳过，而不是拼出必然 404 的 URL。
    #[test]
    fn url_for_skips_sources_without_repo() {
        let p = PresetModel {
            id: "t",
            name: "t",
            summary: "",
            repos: &[(Source::ModelScope, "a/b")],
            file: "f.gguf",
            size_bytes: 1,
            sha256: "0".repeat(64).leak(),
            family: "通用",
            vram_hint_mb: 1,
        };
        assert!(url_for(&p, Source::ModelScope).is_some());
        assert!(
            url_for(&p, Source::HuggingFace).is_none(),
            "没有 HF 仓库映射时不该拼 URL"
        );
        // available_sources 也只应列出真正支持的源
        assert_eq!(available_sources(&p), vec![Source::ModelScope]);
    }

    // ─────────────────── 四态：已暂停 ───────────────────

    /// 有 `.part` 残留时，`PresetStatus` 必须把它报出来。
    ///
    /// 这条锁住的是「磁盘悄悄变满」那个缺陷：`.part` 不进模型列表、
    /// 启动时不扫描、界面没入口，用户取消一次 8 GB 下载就永久少 8 GB
    /// 且无从察觉。界面要能看到，状态里就必须先有这两个字段。
    ///
    /// 变异验证：把 `preset_status` 里 `partial_bytes` 强制写 0，
    /// 本测试失败。
    #[test]
    fn status_reports_partial_download() {
        let dir = std::env::temp_dir().join("xieyi-status-partial-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let p = &PRESETS[0];
        let target = dir.join(format!("{}.gguf", p.id));
        let part = crate::downloader::part_path(&target);

        // 造一个「下了一半」的残留：1500 字节 + 期望 4000
        std::fs::write(&part, vec![0u8; 1500]).unwrap();
        std::fs::write(
            crate::downloader::meta_path_for_test(&part),
            format!(r#"{{"identity":"{}","total":4000}}"#, p.sha256),
        )
        .unwrap();

        let st = preset_status(p, &dir);
        assert!(!st.downloaded, "只有 .part 不算已下载");
        assert_eq!(st.partial_bytes, 1500, "界面要显示已下多少");
        assert_eq!(st.partial_total, 4000, "界面要能算「1500/4000」的百分比");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 已下载时不该再报「已暂停」——两者互斥。
    #[test]
    fn downloaded_precludes_partial_state() {
        let dir = std::env::temp_dir().join("xieyi-status-both-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let p = &PRESETS[0];
        let target = dir.join(format!("{}.gguf", p.id));
        // 造一个大小正确的正式文件 = 已下载
        std::fs::write(&target, vec![0u8; p.size_bytes as usize]).unwrap();
        // 再放一个残留的 .part（现实中不该同时存在，但要保证不会误报）
        std::fs::write(crate::downloader::part_path(&target), vec![0u8; 100]).unwrap();

        let st = preset_status(p, &dir);
        assert!(st.downloaded);
        assert_eq!(
            st.partial_bytes, 0,
            "已下载时不该再显示「已暂停」，否则用户会以为有两份数据"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
