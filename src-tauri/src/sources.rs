//! 下载源抽象：同一个文件，可以从不用的镜像取。
//!
//! ## 为什么需要
//!
//! 2026-11-13 在本机实测三个站点的可达性：
//!
//! | 站点 | 结果 |
//! |---|---|
//! | `huggingface.co` | ❌ 完全不可达（15 秒超时） |
//! | `github.com` | ❌ 完全不可达（15 秒超时） |
//! | `modelscope.cn` | ✅ 可达（0.38 秒） |
//!
//! 也就是说：一个国内用户既下不了推理引擎（走 GitHub），也下不了模型
//! （走 HuggingFace）。这不是"体验差"，是**功能完全不可用**。
//!
//! ## 为什么可以安全地换源
//!
//! **SHA-256 是内容哈希，换源不改变它**。同一个文件从任何镜像取回来，
//! 校验值都一样。这让"多源 + 强校验"这件事变得简单：
//!
//! - 源只影响**从哪拿**
//! - 校验值决定**拿对没有**
//! - 两者解耦，换源不需要改校验逻辑
//!
//! 实测确认魔搭与 HuggingFace 提供相同的 `X-Linked-Etag`（SHA-256）
//! 响应头，且都支持 `Range` 断点续传。
//!
//! ## 不做的事
//!
//! **不做自动测速**。启动时并发请求多个源测延迟，就是一次网络请求——
//! 而本项目的硬约束是「除首次下载推理运行时外，应用不做任何网络请求」。
//!
//! 替代方案（效果一样但不偷偷联网）：
//!
//! 1. 按平台给静态默认值（国内 → 魔搭，海外 → 官方）
//! 2. 记住上次成功的源
//! 3. 失败自动回退到下一个候选
//! 4. 用户随时可手动指定

use serde::{Deserialize, Serialize};

/// GitHub 加速代理的基地址。
///
/// 选它是因为实测最稳（3/3 成功，完整下完 31 MB），且**内容不被篡改**
/// （SHA-256 与官方一致）。其他候选的实测结果见 `Source::GitHubMirror`
/// 的注释。
const GITHUB_MIRROR: &str = "https://gh-proxy.com";

/// 一个下载源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// HuggingFace 官方（海外首选）
    HuggingFace,
    /// 魔搭 ModelScope（国内首选，实测可达）
    ModelScope,
    /// GitHub 官方 release（引擎用）
    GitHub,
    /// GitHub 加速代理（国内用）。
    ///
    /// 实测依据（2026-11-13，本机）：
    ///
    /// | 目标 | 结果 |
    /// |---|---|
    /// | 直连 GitHub 下载 | 4 次里 3 次超时（时通时不通） |
    /// | `gh-proxy.com` | **3/3 成功**，完整下完 31 MB 用时 86 秒 |
    /// | `ghproxy.net` | 3/3 能连但都下不完（7-30 KB/s） |
    ///
    /// **代理不篡改内容**：完整下载后算出的 SHA-256 与官方一致
    /// （`d4765447307f29fc0b4c8ae5b8b806c46b3e515d0ce5c2a54dd90d9b2c7a392f`），
    /// 所以"走代理 + 强校验"是安全的。
    ///
    /// 注意：这个代理**只代理 GitHub**，不代理 HuggingFace（实测 404）。
    GitHubMirror,
    /// 用户自填的镜像基地址。
    ///
    /// ## 为什么需要这个
    ///
    /// 实测（2026-11-13）发现一个尴尬事实：**公共加速代理的可用性极不稳定**。
    /// 同一时刻测了 10 个知名 GitHub 代理：
    ///
    /// | 代理 | 结果 |
    /// |---|---|
    /// | `gh-proxy.com` | ✅ 206 |
    /// | `ghproxy.net` | ❌ 超时 |
    /// | `ghfast.top` | ❌ 403 |
    /// | `gh.ddlc.top` | ❌ 429 |
    /// | 其余 6 个 | ❌ 全部连不上 |
    ///
    /// 也就是说"多列几个内置代理"这条路走不通——今天能用的明天可能挂，
    /// 而写死的列表一旦全挂，用户就彻底卡住。
    ///
    /// 所以留一个**用户可填**的入口：用户有自建反代、公司内网镜像、
    /// 或知道某个当时可用的代理，填进设置即可。这比我们猜哪个能用可靠。
    ///
    /// 地址存在配置的 `customMirror` 键；为空时该源不参与候选。
    Custom,
}

impl Source {
    /// 线格式名，与配置文件和前端共用。
    pub fn as_str(self) -> &'static str {
        match self {
            Source::HuggingFace => "huggingface",
            Source::ModelScope => "modelscope",
            Source::GitHub => "github",
            Source::GitHubMirror => "github-mirror",
            Source::Custom => "custom",
        }
    }

    /// 从配置里的字符串解析，无法识别时返回 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "huggingface" => Some(Source::HuggingFace),
            "modelscope" => Some(Source::ModelScope),
            "github" => Some(Source::GitHub),
            "github-mirror" => Some(Source::GitHubMirror),
            "custom" => Some(Source::Custom),
            _ => None,
        }
    }
}

/// 下载的是哪一类东西。决定默认源的顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 推理引擎（llama.cpp 的 release 包）
    Engine,
    /// 模型文件（.gguf）
    Model,
}

/// 按类别给出**默认源顺序**（第一个是首选）。
///
/// 这里的判断是静态的，不测速。理由见模块文档：测速本身就是网络请求，
/// 与离线优先的定位冲突。
///
/// 顺序的取舍：
///
/// - **引擎**：官方只在 GitHub 发 release，没有等价镜像，所以顺序固定
/// - **模型**：魔搭与 HF 都有；考虑到目标用户多在中文环境，
///   把魔搭排前面
pub fn default_order(kind: Kind) -> &'static [Source] {
    match kind {
        // 引擎只有 GitHub 有 release。直连在国内时通时不通，
        // 所以默认先走代理，官方作为回退。
        Kind::Engine => &[Source::GitHubMirror, Source::GitHub],
        // 模型：国内环境优先魔搭
        Kind::Model => &[Source::ModelScope, Source::HuggingFace],
    }
}

/// 把「仓库 + 文件名」翻译成某个源上的实际 URL。
///
/// `repo` 形如 `tencent/Hunyuan-MT-7B-GGUF`，`file` 是仓库内的文件名。
///
/// 返回 `None` 表示该源不支持这种组合（目前只有引擎不支持模型源）。
pub fn resolve(source: Source, repo: &str, file: &str) -> Option<String> {
    resolve_with(source, repo, file, None)
}

/// 同 [`resolve`]，但可以给 `Source::Custom` 指定基地址。
///
/// 把基地址作为参数而不是去读配置：`resolve` 是**纯函数**，能直接单测。
/// 读配置那层放在调用方（`candidates_for` 之类）。
pub fn resolve_with(
    source: Source,
    repo: &str,
    file: &str,
    custom_base: Option<&str>,
) -> Option<String> {
    let url = match source {
        Source::HuggingFace => {
            format!("https://huggingface.co/{repo}/resolve/main/{file}")
        }
        // 魔搭的分支名是 master，不是 main
        Source::ModelScope => {
            format!("https://modelscope.cn/models/{repo}/resolve/master/{file}")
        }
        Source::GitHub => {
            // 引擎的 URL 由 runtime.rs 拼（要带 pin 的版本号与平台名），
            // 这里只处理通用的 owner/repo/releases 形式
            format!("https://github.com/{repo}/releases/download/{file}")
        }
        // 加速代理的用法是「把完整 GitHub URL 接在代理域名后面」，
        // 不是替换域名。实测 gh-proxy.com 支持这种形式。
        // 用户自填的镜像：同样是「基地址 + 完整 GitHub URL」的前缀式用法。
        // 不采用"替换域名"是因为镜像实现各异，前缀式最通用。
        Source::Custom => {
            // 先 trim 再判空：用户可能只填了空格，那等于没填
            let base = custom_base?.trim().trim_end_matches('/');
            if base.is_empty() {
                return None;
            }
            format!("{base}/https://github.com/{repo}/releases/download/{file}")
        }
        Source::GitHubMirror => {
            format!(
                "{}/{}/{}",
                GITHUB_MIRROR,
                "https://github.com",
                format_args!("{repo}/releases/download/{file}")
            )
        }
    };
    Some(url)
}

/// 按顺序尝试的源列表，第一个是首选。
///
/// `preferred` 来自用户配置或上次成功的记录；为 `None` 时用默认顺序。
pub fn candidates(kind: Kind, preferred: Option<Source>) -> Vec<Source> {
    let base = default_order(kind);
    let Some(pref) = preferred else {
        return base.to_vec();
    };

    // 首选放最前，其余保持原顺序（去重）
    let mut out = vec![pref];
    for s in base {
        if *s != pref {
            out.push(*s);
        }
    }
    out
}

/// 配置键名按类别分开。
///
/// 为什么不能共用一个键：引擎的候选是「加速代理 / 官方」，模型的是
/// 「魔搭 / HF」。共用一个 `preferredSource` 会让两者互相覆盖——
/// 用户把模型偏好设成 HF，引擎那边也会被读成 HF（它并不是一个
/// 引擎源），于是偏好失效。
fn preferred_key(kind: Kind) -> &'static str {
    match kind {
        Kind::Engine => "preferredSourceEngine",
        Kind::Model => "preferredSourceModel",
    }
}

/// 读某一类的偏好源。
pub fn preferred_for(app: &tauri::AppHandle, kind: Kind) -> Option<Source> {
    crate::paths::config_value(app, preferred_key(kind))
        .and_then(|v| v.as_str().map(String::from))
        .and_then(|s| Source::parse(&s))
}

/// 记住用户这次用成功的源。
///
/// 下次下载优先用它——这是"默认选最快的"的替代实现：不测速，
/// 而是**用上次真正成功过的那个**。
pub fn remember_source(app: &tauri::AppHandle, source: Source) -> Result<(), String> {
    remember_source_for(app, Kind::Model, source)
}

/// 记住某一类下载的偏好源。
pub fn remember_source_for(
    app: &tauri::AppHandle,
    kind: Kind,
    source: Source,
) -> Result<(), String> {
    crate::paths::set_config_value(
        app,
        preferred_key(kind),
        serde_json::Value::String(source.as_str().to_string()),
    )
}

/// 清除某一类的偏好，恢复默认顺序。
pub fn clear_preferred(app: &tauri::AppHandle, kind: Kind) -> Result<(), String> {
    crate::paths::set_config_value(app, preferred_key(kind), serde_json::Value::Null)
}

/// 按用户偏好排好的候选源列表，供下载器逐个尝试。
pub fn candidates_for(app: &tauri::AppHandle, kind: Kind) -> Vec<Source> {
    let mut list = candidates(kind, preferred_for(app, kind));

    // 用户填了自定义镜像就把它接在最后作为最终兜底。
    //
    // 放在**最后**而不是最前：内置代理与官方是经过验证的，用户填的
    // 地址未必可用（可能打错、可能已失效）。让验证过的先上，避免
    // 一个坏地址把用户卡在第一顺位。
    if custom_mirror(app).is_some() && !list.contains(&Source::Custom) {
        list.push(Source::Custom);
    }
    list
}

/// 读用户自填的 GitHub 镜像基地址。
///
/// 键名 `customMirror`。空值视为未设置。
pub fn custom_mirror(app: &tauri::AppHandle) -> Option<String> {
    crate::paths::config_value(app, "customMirror")
        .and_then(|v| v.as_str().map(String::from))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 读取/保存自定义镜像（供前端设置页调用）。
#[tauri::command]
pub fn get_download_source_config(
    app: tauri::AppHandle,
    kind: String,
) -> Result<SourceConfig, String> {
    let k = parse_kind(&kind)?;
    Ok(SourceConfig {
        kind: kind.clone(),
        preferred: preferred_for(&app, k).map(|s| s.as_str().to_string()),
        custom_mirror: custom_mirror(&app),
    })
}

/// 保存下载源配置。
///
/// `kind` 为 `engine` 或 `model`——两类下载的候选源不同，偏好也分开存。
/// `preferred` 传空表示恢复默认顺序。
#[tauri::command]
pub fn set_download_source_config(
    app: tauri::AppHandle,
    kind: String,
    preferred: Option<String>,
    custom_mirror: Option<String>,
) -> Result<(), String> {
    let k = parse_kind(&kind)?;

    match preferred
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(name) => {
            let src = Source::parse(name).ok_or_else(|| format!("无法识别的下载源：{name}"))?;
            // 校验：这个源对该类别是否有意义。否则用户存了个模型源却
            // 指望它影响引擎，结果静默不生效。
            if !default_order(k).contains(&src) && src != Source::Custom {
                return Err(format!(
                    "{} 不支持把 {name} 作为下载源",
                    match k {
                        Kind::Engine => "引擎",
                        Kind::Model => "模型",
                    }
                ));
            }
            remember_source_for(&app, k, src)?;
        }
        // 清除偏好，恢复默认顺序
        None => clear_preferred(&app, k)?,
    }

    if let Some(custom) = custom_mirror {
        set_custom_mirror(&app, &custom)?;
    }
    Ok(())
}

/// 把前端传来的类别字符串转成 `Kind`。
fn parse_kind(s: &str) -> Result<Kind, String> {
    match s {
        "engine" => Ok(Kind::Engine),
        "model" => Ok(Kind::Model),
        other => Err(format!("未知的下载类别：{other}")),
    }
}

/// 下载源配置，供设置页读写。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceConfig {
    /// 类别：`engine` 或 `model`
    pub kind: String,
    /// 用户偏好的源（线格式名）；`None` 表示用默认顺序
    pub preferred: Option<String>,
    /// 自填的镜像基地址
    pub custom_mirror: Option<String>,
}

/// 保存用户自填的镜像。传空字符串表示清除。
pub fn set_custom_mirror(app: &tauri::AppHandle, base: &str) -> Result<(), String> {
    let trimmed = base.trim();
    // 只接受 http(s)：避免用户填个本地文件路径导致奇怪行为
    if !trimmed.is_empty() && !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err("镜像地址必须以 http:// 或 https:// 开头".into());
    }
    crate::paths::set_config_value(
        app,
        "customMirror",
        serde_json::Value::String(trimmed.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两类下载的偏好必须分开存，不能互相覆盖。
    ///
    /// 早先共用一个 `preferredSource` 键。用户把模型偏好设成
    /// HuggingFace 后，引擎那边也读成 HuggingFace（它不是引擎源），
    /// 于是引擎的偏好静默失效。
    #[test]
    fn preferred_keys_are_separate_per_kind() {
        assert_ne!(
            preferred_key(Kind::Engine),
            preferred_key(Kind::Model),
            "引擎与模型的偏好必须存在不同键里"
        );
    }

    /// 引擎不能把模型源当作偏好——存了也不生效，属于静默失败。
    #[test]
    fn engine_rejects_model_sources() {
        // 这两个都不在引擎的候选里
        assert!(!default_order(Kind::Engine).contains(&Source::ModelScope));
        assert!(!default_order(Kind::Engine).contains(&Source::HuggingFace));
        // 反过来也一样
        assert!(!default_order(Kind::Model).contains(&Source::GitHub));
        assert!(!default_order(Kind::Model).contains(&Source::GitHubMirror));
    }

    /// 类别字符串解析要能识别前端传来的两种值。
    #[test]
    fn kind_parses_from_wire_names() {
        assert_eq!(parse_kind("engine"), Ok(Kind::Engine));
        assert_eq!(parse_kind("model"), Ok(Kind::Model));
        assert!(parse_kind("unknown").is_err());
        assert!(parse_kind("").is_err());
    }

    /// 自定义镜像：前缀式，且必须能去除末尾多余的斜杠。
    #[test]
    fn custom_mirror_builds_prefixed_url() {
        let url = resolve_with(
            Source::Custom,
            "ggml-org/llama.cpp",
            "b11714/x.zip",
            Some("https://my.cache:8080/"),
        )
        .unwrap();
        assert_eq!(
            url,
            "https://my.cache:8080/https://github.com/ggml-org/llama.cpp/releases/download/b11714/x.zip"
        );
    }

    /// 没给基地址时不能拼出半截 URL，应返回 None 让调用方跳过。
    #[test]
    fn custom_mirror_without_base_is_rejected() {
        assert!(resolve_with(Source::Custom, "o/r", "f", None).is_none());
        assert!(resolve_with(Source::Custom, "o/r", "f", Some("")).is_none());
        assert!(resolve_with(Source::Custom, "o/r", "f", Some("   ")).is_none());
    }

    /// 自定义源默认**不**在候选里——用户没填就不该尝试。
    #[test]
    fn custom_is_not_in_default_order() {
        assert!(!default_order(Kind::Engine).contains(&Source::Custom));
        assert!(!default_order(Kind::Model).contains(&Source::Custom));
    }

    #[test]
    fn source_round_trips_through_str() {
        for s in [
            Source::HuggingFace,
            Source::ModelScope,
            Source::GitHub,
            Source::GitHubMirror,
            Source::Custom,
        ] {
            assert_eq!(Source::parse(s.as_str()), Some(s), "{:?}", s);
        }
        assert_eq!(Source::parse("unknown"), None);
        assert_eq!(Source::parse(""), None);
    }

    /// 引擎只有 GitHub 系来源——官方不在别的平台发 release。
    ///
    /// 默认先走加速代理：实测直连「时通时不通」（4 次里 3 次超时），
    /// 而 gh-proxy.com 3/3 成功。官方保留为回退。
    #[test]
    fn engine_prefers_mirror_then_official() {
        let order = default_order(Kind::Engine);
        assert_eq!(order[0], Source::GitHubMirror, "国内环境应先试加速");
        assert_eq!(order[1], Source::GitHub, "官方必须作为回退保留");
        // 引擎没有模型源可用
        assert!(!order.contains(&Source::ModelScope));
        assert!(!order.contains(&Source::HuggingFace));
    }

    /// 代理 URL 必须是「代理域名 + 完整 GitHub URL」的形式。
    ///
    /// 实测 gh-proxy.com 支持这种前缀式用法，而不是替换域名。
    #[test]
    fn mirror_url_prefixes_the_full_github_url() {
        let url = resolve(Source::GitHubMirror, "ggml-org/llama.cpp", "b11714/x.zip").unwrap();
        assert!(url.starts_with("https://gh-proxy.com/"), "{url}");
        assert!(
            url.contains("https://github.com/ggml-org/llama.cpp/releases/download/"),
            "代理后面必须接完整 GitHub 地址：{url}"
        );
        assert!(url.ends_with("b11714/x.zip"), "{url}");
    }

    /// 代理与官方指向同一个文件（只是路径不同），这是"换源不改校验"的前提。
    #[test]
    fn mirror_and_official_refer_to_same_file() {
        let official = resolve(Source::GitHub, "o/r", "v/f.zip").unwrap();
        let mirror = resolve(Source::GitHubMirror, "o/r", "v/f.zip").unwrap();
        assert_ne!(official, mirror);
        assert!(
            mirror.ends_with(&official),
            "代理 URL 应以内层官方 URL 结尾"
        );
    }

    /// 模型默认优先魔搭：目标用户多在中文环境，实测魔搭可达而 HF 不可达。
    #[test]
    fn model_defaults_to_modelscope_first() {
        let order = default_order(Kind::Model);
        assert_eq!(order[0], Source::ModelScope);
        assert!(order.contains(&Source::HuggingFace), "必须保留 HF 作为回退");
    }

    /// 用户指定的源排最前，其余作为回退保留。
    #[test]
    fn preferred_source_goes_first() {
        let c = candidates(Kind::Model, Some(Source::HuggingFace));
        assert_eq!(c[0], Source::HuggingFace);
        assert!(c.contains(&Source::ModelScope), "回退源不能丢");
    }

    /// 首选已在默认顺序里时不能重复。
    #[test]
    fn candidates_are_deduplicated() {
        let c = candidates(Kind::Model, Some(Source::ModelScope));
        assert_eq!(c.len(), 2, "不应出现重复项：{c:?}");
        assert_eq!(c.iter().filter(|s| **s == Source::ModelScope).count(), 1);
    }

    #[test]
    fn no_preference_uses_default_order() {
        assert_eq!(
            candidates(Kind::Model, None),
            default_order(Kind::Model).to_vec()
        );
    }

    /// 两个源的 URL 必须不同——否则"换源"是假的。
    #[test]
    fn different_sources_give_different_urls() {
        let hf = resolve(Source::HuggingFace, "a/b", "c.gguf").unwrap();
        let ms = resolve(Source::ModelScope, "a/b", "c.gguf").unwrap();
        assert_ne!(hf, ms);
        assert!(hf.starts_with("https://huggingface.co/"));
        assert!(ms.starts_with("https://modelscope.cn/"));
    }

    /// 魔搭的分支名是 master，HF 是 main。写错会 404。
    #[test]
    fn branch_names_match_each_provider() {
        let hf = resolve(Source::HuggingFace, "r", "f").unwrap();
        let ms = resolve(Source::ModelScope, "r", "f").unwrap();
        assert!(hf.contains("/resolve/main/"), "{hf}");
        assert!(ms.contains("/resolve/master/"), "{ms}");
    }

    /// 仓库名与文件名必须原样出现在 URL 里，不能被转义或截断。
    #[test]
    fn repo_and_file_are_preserved() {
        let url = resolve(
            Source::HuggingFace,
            "tencent/Hunyuan-MT-7B-GGUF",
            "Hy-MT2-1.8B-Q8_0.gguf",
        )
        .unwrap();
        assert!(url.contains("tencent/Hunyuan-MT-7B-GGUF"));
        assert!(url.ends_with("Hy-MT2-1.8B-Q8_0.gguf"));
    }
}
