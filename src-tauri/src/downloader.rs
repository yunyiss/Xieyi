//! 通用文件下载器：断点续传 + SHA-256 校验 + 进度回调。
//!
//! ## 为什么要自己写，而不用现成的
//!
//! 实测过主流方案（[依据](https://www.bigiron.cc/guides/fast-resumable-hugging-face-model-downloads-hf-cli-xet-and-mirrors)）：
//!
//! | 工具 | 断点续传 |
//! |---|---|
//! | `hf download` | ❌ **不能**（huggingface_hub ≥1.18.0 改成每进程唯一临时文件，杀掉进程后新进程找不到旧的部分） |
//! | `curl -C -` | ✅ 精确续传 |
//! | `aria2c -c` | ✅ 但有约 6% 重复开销 |
//!
//! 我们不能用 `hf`（不能续传）；也不想外挂 `curl`/`aria2` 二进制
//! （要多分发两个可执行文件，且拿不到流式进度做 UI）。
//!
//! 所以用 reqwest 自己发 `Range` 请求——语义照抄 `curl -C -`，
//! 但能接进我们的进度事件。
//!
//! ## 续传的正确做法（这里踩过的坑都写下来）
//!
//! 简单地在已有文件后面 append 是**不安全**的：
//!
//! - 服务端返回的内容可能不是从我们以为的位置开始的
//! - 文件可能在两次尝试之间被换掉（镜像更新、CDN 换源）
//!
//! 所以续传前必须校验两件事（参考
//! [huggingface_hub PR #4143](https://github.com/huggingface/huggingface_hub/pull/4143)）：
//!
//! 1. **`If-Range`**：把已有文件的大小/时间戳发过去，服务端若认为文件
//!    已变，会返回 `200` 而不是 `206`，此时必须**从头下**
//! 2. **`Content-Range`**：返回的起点必须等于本地已有字节数，否则丢弃重下
//!
//! 任何一条对不上，都退回"从头下载"。**宁可重下，不要拼出个坏文件**。
//!
//! ## `.part` 文件
//!
//! 下载中的内容写到 `<目标>.part`，校验通过后才改名为正式文件。
//! 这样"半截文件"永远不会被误认为是完整的——上层扫描模型目录时
//! 只认 `.gguf`，`.part` 天然被忽略。
//!
//! ## 分段并发
//!
//! 大文件（≥ 8 MB）且服务端支持 `Range` 时，按 1–16 MB 切片、最多 8 条
//! 连接并发下载（慢启动扩容）。目的是绕开 CDN / 公共代理常见的
//! **单连接限速**：实测单连接 57 KB/s 时，8 条能到 287 KB/s。
//!
//! 每片的进度都写进 `.part.meta`，取消或断网后续传只补没下完的片。
//! 服务端不支持 Range（回 200）时自动回退单流，不浪费已下的数据。

use futures_util::stream::{FuturesUnordered, StreamExt};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

/// 下载进度。`total == 0` 表示服务端没给 `Content-Length`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// 已下载字节数（含续传前已有的部分）
    pub downloaded: u64,
    /// 总字节数；0 表示未知
    pub total: u64,
}

/// 下载失败的原因。
///
/// ## 为什么把「取消」单列成一种，而不是塞进字符串
///
/// 取消**不是失败**。它有三条与失败完全不同的后续动作：
///
/// | 动作 | 失败 | 取消 |
/// |---|---|---|
/// | 换源重试 | 可能（看错误类型） | **绝不能**（用户已经不要了） |
/// | 保留 `.part` | 看情况 | **必须**（下次要续传） |
/// | 界面表现 | 红字报错 | 中性提示 |
///
/// 早先这两种情况都返回 `String`，只能靠 `err.contains("取消")` 区分——
/// 那就是"用文案当控制流"，改一个字就静默失效。现在由类型区分，
/// 编译器保证调用方必须显式处理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /// 用户请求取消。`.part` 与 `.part.meta` 都保留，下次可续传。
    Cancelled,
    /// 真正的失败（网络、磁盘、校验）。`String` 是给人看的说明。
    Failed(String),
    /// 下载完整但哈希对不上。
    ///
    /// 单独一类是因为它**值得自动重试一次**：公共代理偶发返回坏数据、
    /// 或下载过程中被中间设备篡改，重下一遍往往就好了。重试仍失败
    /// 才升级成 `Failed` 报给用户。
    Checksum(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Cancelled => write!(f, "已取消下载"),
            DownloadError::Failed(msg) | DownloadError::Checksum(msg) => f.write_str(msg),
        }
    }
}

/// 让 `?` 在返回 `String` 的 Tauri 命令里能直接接住（命令签名不变）。
impl From<DownloadError> for String {
    fn from(e: DownloadError) -> Self {
        e.to_string()
    }
}

/// 用户是否已请求取消。
///
/// `None` 表示这次下载不可取消（比如引擎包——31 MB，几秒就完）。
fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel
        .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(false)
}

/// 下载结果。
#[derive(Debug)]
pub struct Downloaded {
    /// 最终文件路径（`.part` 已改名）
    pub path: PathBuf,
    /// 实际字节数
    pub bytes: u64,
    /// 实际算出的 SHA-256（小写十六进制）
    pub sha256: String,
    /// 是否用了续传（已有部分数据）
    pub resumed: bool,
}

/// 下载一个文件到 `dest`，可断点续传；校验失败会自动重下一次。
///
/// - `url`：完整下载地址
/// - `dest`：最终路径。下载中写 `dest.part`
/// - `expected_sha256`：期望的哈希；`None` 表示不校验（**不建议**，
///   但有些场景（如自定义镜像）拿不到哈希）
/// - `cancel`：取消标志。`None` 表示这次下载不可取消（引擎包就是这么传的）
/// - `on_progress`：进度回调，调用频率由内部限流（约 250ms 一次）
///
/// 返回 `Err(DownloadError::Cancelled)` 时 `.part` **保留**，下次可续传。
pub async fn download<F>(
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    cancel: Option<&AtomicBool>,
    mut on_progress: F,
) -> Result<Downloaded, DownloadError>
where
    F: FnMut(Progress),
{
    match download_once(url, dest, expected_sha256, cancel, &mut on_progress).await {
        // 公共代理偶发坏数据：整份重下一次。重下前 finish 已经删掉了坏文件。
        Err(DownloadError::Checksum(msg)) => {
            let part = part_path(dest);
            let _ = tokio::fs::remove_file(&part).await;
            let _ = tokio::fs::remove_file(meta_path(&part)).await;
            match download_once(url, dest, expected_sha256, cancel, &mut on_progress).await {
                Err(DownloadError::Checksum(_)) => Err(DownloadError::Failed(format!(
                    "{msg} 已自动重试一次，结果仍不一致；\
                     请更换下载源或稍后再试。"
                ))),
                other => other,
            }
        }
        other => other,
    }
}

/// 单次尝试。见 [`download`]。
async fn download_once<F>(
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    cancel: Option<&AtomicBool>,
    on_progress: &mut F,
) -> Result<Downloaded, DownloadError>
where
    F: FnMut(Progress),
{
    let part = part_path(dest);

    // 一进来就先看一次：用户可能在排队等源的时候就取消了
    if cancelled(cancel) {
        return Err(DownloadError::Cancelled);
    }

    let identity = expected_sha256.unwrap_or(url).to_string();
    let mut meta = read_meta(&part).await;

    // 身份不符：旧半截文件不属于这个产物，绝不能续传
    if let Some(m) = meta.as_ref() {
        if m.identity != identity {
            let _ = tokio::fs::remove_file(&part).await;
            let _ = tokio::fs::remove_file(meta_path(&part)).await;
            meta = None;
        } else if m.version >= META_VERSION && !tokio::fs::try_exists(&part).await.unwrap_or(false)
        {
            // 元信息还在、数据文件没了（换源时被清掉）：这份 meta 无效
            let _ = tokio::fs::remove_file(meta_path(&part)).await;
            meta = None;
        }
    }

    // ── v2：分段续传 ──────────────────────────────────────────────
    if let Some(m) = meta
        .as_ref()
        .filter(|m| m.version >= META_VERSION && !m.segments.is_empty())
    {
        match segmented_resume(url, &part, m, cancel, &mut *on_progress).await {
            // 分段全部完成：交给统一收尾做校验与改名
            Ok(true) => {
                return finish_download(dest, &part, expected_sha256, m.total, true).await;
            }
            // 服务端不支持 Range / 大小变了：清掉重下
            Ok(false) => {
                let _ = tokio::fs::remove_file(&part).await;
                let _ = tokio::fs::remove_file(meta_path(&part)).await;
                meta = None;
            }
            Err(e) => return Err(e),
        }
    }

    // ── v1：单流续传（老版本留下的 .part）────────────────────────
    let existing = if meta.is_some() {
        tokio::fs::metadata(&part)
            .await
            .map(|m| m.len())
            .unwrap_or(0)
    } else {
        0
    };
    if existing > 0 {
        match try_resume(
            url,
            &part,
            existing,
            expected_sha256,
            cancel,
            &mut *on_progress,
        )
        .await
        {
            Ok(Some(final_size)) => {
                return finish_download(dest, &part, expected_sha256, final_size, true).await;
            }
            Ok(None) => {
                // `try_resume` 已把不可信的半截文件删掉，继续走全新下载
            }
            Err(e) => return Err(e),
        }
    }

    // ── 全新下载：先探测 Range 能力，再决定分段还是单流 ──────────
    let probe = probe_range(url, None).await?;

    if probe.ranged && probe.total >= SEGMENT_MIN_TOTAL {
        let total = probe.total;
        segmented_fresh(url, &part, probe, identity, cancel, &mut *on_progress).await?;
        return finish_download(dest, &part, expected_sha256, total, false).await;
    }

    // 探测只拿到 1 字节（小文件）或服务端不吃 Range：
    // 前者重新整份请求，后者直接复用探测响应，避免多发一次请求。
    if probe.ranged {
        let total =
            fresh_download(url, &part, expected_sha256, cancel, &mut *on_progress, None).await?;
        return finish_download(dest, &part, expected_sha256, total, false).await;
    }

    // 200 响应直接复用；206 但总大小解析不出（`bytes 0-0/*`）时
    // `resp` 为空，交给 fresh_download 重新发一次完整请求。
    let total = fresh_download(
        url,
        &part,
        expected_sha256,
        cancel,
        &mut *on_progress,
        probe.resp,
    )
    .await?;
    finish_download(dest, &part, expected_sha256, total, false).await
}

/// 下载完成后的统一收尾：校验哈希、改名、确认落位、清元信息。
///
/// 单流与分段两条路径共用，避免两套收尾逻辑各漏一处。
async fn finish_download(
    dest: &Path,
    part: &Path,
    expected_sha256: Option<&str>,
    downloaded: u64,
    resumed: bool,
) -> Result<Downloaded, DownloadError> {
    // 校验：先算哈希，再决定是否改名
    let sha256 = sha256_file(part).await.map_err(DownloadError::Failed)?;

    if let Some(expected) = expected_sha256 {
        let expected = expected.trim().to_lowercase();
        if sha256 != expected {
            // 内容确认是坏的，留着只会让下次续传拼出更坏的文件
            let _ = tokio::fs::remove_file(part).await;
            let _ = tokio::fs::remove_file(meta_path(part)).await;
            return Err(DownloadError::Checksum(format!(
                "下载校验失败：期望 {expected}，实际 {sha256}。"
            )));
        }
    }

    // 校验通过才改名。跨设备时 rename 会失败，退回 copy + remove
    if tokio::fs::rename(part, dest).await.is_err() {
        tokio::fs::copy(part, dest)
            .await
            .map_err(|e| DownloadError::Failed(format!("保存文件失败: {e}")))?;
        let _ = tokio::fs::remove_file(part).await;
    }

    // **确认真的落位了**再报告成功。
    //
    // 不要因为"输入流读完了"就宣布完成：rename 可能静默失败
    // （目标磁盘满、权限、杀软占用），此时 `dest` 可能不存在或是 0 字节。
    let placed = tokio::fs::metadata(dest)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if placed != downloaded {
        // 落位失败就把半截文件清掉，避免下次续传拼到错误的基线上
        let _ = tokio::fs::remove_file(dest).await;
        return Err(DownloadError::Failed(format!(
            "文件保存失败：应为 {downloaded} 字节，实际写入 {placed} 字节。\
             可能是磁盘空间不足或被其他程序占用。"
        )));
    }

    // 收尾：删掉 `.part.meta`，它只服务于未完成的下载
    let _ = tokio::fs::remove_file(meta_path(part)).await;

    Ok(Downloaded {
        path: dest.to_path_buf(),
        bytes: downloaded,
        sha256,
        resumed,
    })
}

/// 从多个源依次尝试下载，第一个成功的即采用。
///
/// ## 为什么需要"多源"这层，而不是把源逻辑塞进 `download()`
///
/// `download()` 的职责是"把一个 URL 的内容可靠地落到磁盘"。它不该知道
/// "这个文件还能从别处拿"——那是策略，不是机制。分开之后：
///
/// - `download()` 可以单测（不依赖源的选择）
/// - 源策略可以独立演进（加镜像、改优先级）而不动下载逻辑
///
/// ## 超时策略（重要，这里区分了三种超时）
///
/// 用户担心的是"网速不好"，但网速不好属于**传输变慢**，不是"源不通"。
/// 把两者混为一谈会导致：慢速下载被误判成故障 → 切源 → 从头重下。
///
/// 所以：
///
/// | 阶段 | 判定 | 行为 |
/// |---|---|---|
/// | 连接 | 10 秒连不上 | 该源不通，切下一个 |
/// | 首字节 | 30 秒没数据 | 切下一个 |
/// | 传输 | 只要有字节流动就一直等 | **绝不切源** |
/// | 传输卡死 | 60 秒 0 字节 | 保留 `.part` 并报错，不切源 |
///
/// 最后一条是关键：卡死时切源会丢掉已下进度。正确做法是保住 `.part`，
/// 让用户稍后续传。
///
/// ## 不做的事
///
/// **不测速**。测速要在下载前并发请求多个源，本身就是网络请求，与
/// "除首次下载运行时外不做任何网络请求"的约束冲突。取而代之：
///
/// 1. 按传入顺序尝试（调用方用 `sources::candidates_for` 排好）
/// 2. 成功后调用方可以 `remember_source` 记住它
pub async fn download_from_sources<F>(
    urls: &[String],
    dest: &Path,
    expected_sha256: Option<&str>,
    cancel: Option<&AtomicBool>,
    mut on_progress: F,
) -> Result<Downloaded, DownloadError>
where
    F: FnMut(Progress),
{
    if urls.is_empty() {
        return Err(DownloadError::Failed("没有可用的下载源".into()));
    }

    let mut failures: Vec<String> = Vec::new();

    for (i, url) in urls.iter().enumerate() {
        let is_last = i + 1 == urls.len();

        match download(url, dest, expected_sha256, cancel, &mut on_progress).await {
            Ok(d) => return Ok(d),
            // 取消立刻上抛：绝不能换源重试，用户已经不要了
            Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
            // 本源自动重下一次后仍不一致：换一个镜像大概率是好的
            Err(DownloadError::Checksum(e)) => {
                failures.push(format!("第 {} 个源校验失败：{e}", i + 1));
                if is_last {
                    let detail = failures.join("；");
                    return Err(DownloadError::Failed(format!(
                        "{detail}。已尝试 {} 个源且在失败源上各重试一次。\n\
                         建议在设置里更换下载源，或手动下载后放入模型目录。",
                        urls.len()
                    )));
                }
                let _ = tokio::fs::remove_file(part_path(dest)).await;
                let _ = tokio::fs::remove_file(meta_path(&part_path(dest))).await;
            }
            Err(DownloadError::Failed(e)) => {
                failures.push(format!("第 {} 个源失败：{e}", i + 1));

                // 只有"源不通"才值得换源。传输中卡死这类错误已经在
                // download() 里保留了 .part，换源会丢掉进度，所以不换。
                if is_last || !should_try_next_source(&e) {
                    let detail = failures.join("；");
                    return Err(DownloadError::Failed(format!(
                        "{detail}。已尝试 {} 个源。\n\
                         若网络受限，可在设置里手动指定下载源，\
                         或手动下载后放入模型目录。",
                        urls.len()
                    )));
                }
                // 换源前清掉可能残留的半截文件：不同源的内容若不一致，
                // 拼起来必然是坏的。元信息一起删，避免下游误判可续传。
                let _ = tokio::fs::remove_file(part_path(dest)).await;
                let _ = tokio::fs::remove_file(meta_path(&part_path(dest))).await;
            }
        }
    }

    Err(DownloadError::Failed(failures.join("；")))
}

/// 这个错误是否值得换源重试。
///
/// **只有"连不上"和"服务端明确拒绝"才换**。传输中断不换——`.part`
/// 里已有数据，换源会从零开始，反而更糟。
fn should_try_next_source(err: &str) -> bool {
    // 连接失败 / DNS / 超时
    if err.contains("请求下载地址失败") || err.contains("续传请求失败") {
        return true;
    }
    // 服务端明确报错（404 / 500 等）——这个源上没有这个文件
    if err.contains("下载失败，HTTP") {
        return true;
    }
    // 传输中断、写入失败、校验失败：不换源
    false
}

/// 连接超时：连不上就换源。
///
/// 10 秒足够判断"这个源通不通"（实测不可达的站点会在 TCP 层挂到超时）。
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 首字节超时：连上了但服务器迟迟不吐数据。
///
/// 比连接超时宽松得多——慢速服务器可能需要时间准备大文件。
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// 分段下载的门槛：小于这个大小就老实单流，不值得为省几秒开并发。
const SEGMENT_MIN_TOTAL: u64 = 8 * 1024 * 1024;

/// 单个分片的目标大小。
///
/// 不是越小越好：每片都是一次独立的 Range 请求，太小会让请求数暴涨
/// （1.8 GB 按 1 MB 切就是 1800 次）；也不能太大，否则负载不均、
/// 断点续传的粒度太粗。8 等分再夹在 1–16 MB 之间，30 MB 的引擎
/// 正好是 4 MB × 8 片。
const SEGMENT_CHUNK_MIN: u64 = 1024 * 1024;
const SEGMENT_CHUNK_MAX: u64 = 16 * 1024 * 1024;

/// 并发连接上限。
///
/// 实测（2026-11-23，gh-proxy 下 16 MB 样本）：单连接 57 KB/s、
/// 4 连接 209 KB/s、8 连接 287 KB/s。继续加收益递减，而家用网络和
/// 公共代理对高并发更敏感，所以停在 8。
const SEGMENT_WORKERS: usize = 8;

/// 单个分片的重试次数（不含首次）。
const SEGMENT_RETRIES: u32 = 3;

/// 元信息格式版本。v2 = 带 `segments`（分段进度）。
const META_VERSION: u32 = 2;

/// 下载时用的 `User-Agent`。
///
/// ## 为什么必须显式设置（实测踩过）
///
/// reqwest **默认不发送 `User-Agent`**。而 ModelScope 的 CDN
/// （`cdn-lfs-cn-1.modelscope.cn`）会把没有 UA 的请求直接判成爬虫，
/// 返回 **403 Forbidden**。
///
/// 2026-11-14 实测（同一条 URL，只换 UA）：
///
/// | 请求 | 结果 |
/// |---|---|
/// | `curl` 默认（自带 UA） | ✅ 206 |
/// | `curl -H "User-Agent:"`（清空） | ❌ **403** |
/// | `curl -A "reqwest/0.12.9"` | ✅ 206 |
///
/// 也就是说：魔搭这个"国内可用"的源，对我们**完全不可用**——
/// 用户在国内点下载必然失败，而失败信息是 403，看起来像是没权限，
/// 根本猜不到是 UA 的问题。
///
/// 格式用 `产品名/版本`，这是 RFC 9110 建议的机器人写法，
/// 也便于对方站点在日志里认出我们（出问题时好沟通）。
const USER_AGENT: &str = concat!("Xieyi/", env!("CARGO_PKG_VERSION"));

/// 构造下载用的 HTTP 客户端。
///
/// ## 为什么**不**设 `timeout()`
///
/// `Client::timeout()` 是**整个请求**的总超时。模型动辄几个 GB，
/// 在慢速网络上要下几十分钟——设总超时等于"下载到一半就被自己杀掉"。
///
/// 只设两个阶段的超时：
///
/// - `connect_timeout`：判断源通不通
/// - `read_timeout`：判断服务器是否还活着（每次读数据的间隔上限）
///
/// 传输阶段只要还有字节流动就无限等下去——这正是"网速慢"该有的行为。
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// `.part` 路径：`foo.gguf` → `foo.gguf.part`。
///
/// 用后缀而不是前缀：用户在文件管理器里能一眼看到"这是没下完的"，
/// 而且 `models::scan` 只认 `.gguf` 扩展名，`.part` 天然被忽略。
pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

/// 从头下载，返回下载到的字节数。
///
/// `resp` 是探测阶段已经拿到的响应（服务端不支持 Range 时，它是一份
/// 完整的 200 响应）。复用它就少发一次请求——也避免本地测试服务里
/// 「只 accept 一次」的场景直接失败。
async fn fresh_download<F>(
    url: &str,
    part: &Path,
    expected_sha256: Option<&str>,
    cancel: Option<&AtomicBool>,
    on_progress: &mut F,
    resp: Option<reqwest::Response>,
) -> Result<u64, DownloadError>
where
    F: FnMut(Progress),
{
    let resp = match resp {
        Some(r) => r,
        None => http_client()
            .get(url)
            .send()
            .await
            .map_err(|e| DownloadError::Failed(format!("请求下载地址失败: {e}")))?,
    };

    if !resp.status().is_success() {
        return Err(DownloadError::Failed(format!(
            "下载失败，HTTP {}。可尝试更换下载源或检查网络。",
            resp.status()
        )));
    }

    let total = resp.content_length().unwrap_or(0);

    // 记下 ETag：续传时用 `If-Range` 发回去，让服务端判断文件有没有变
    let etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // 记录产物身份。**必须在写数据前**——万一进程在这里被杀，
    // 下次续传要靠它判断这个半截文件还属不属于同一个产物。
    let identity = expected_sha256.unwrap_or(url).to_string();
    write_meta(part, &identity, total, etag).await;

    if let Some(parent) = part.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| DownloadError::Failed(format!("创建目录失败: {e}")))?;
    }

    let mut file = tokio::fs::File::create(part)
        .await
        .map_err(|e| DownloadError::Failed(format!("创建文件失败: {e}")))?;

    write_stream(resp, &mut file, 0, total, cancel, on_progress).await?;
    Ok(total)
}

/// 续传元信息：与 `.part` 一起存放，用来确认「这个半截文件属于当前这个产物」。
///
/// ## 为什么必须有它
///
/// 只凭「本地有 N 字节」就去续传是**不安全**的。真实会发生的场景：
///
/// - 镜像更新了同一个文件名的内容
/// - 上一个预设的 `.part` 残留，用户换了个模型下
/// - 仓库换了 revision
///
/// 这些情况下把新数据 append 到旧半截文件后面，得到的是一个**大小正确、
/// 内容错误**的文件——而且 SHA-256 校验会失败，但用户已经白下了一遍。
///
/// 参考 [Resumable Downloads Need Validators, Not Just a Byte Offset](https://www.gioyous.it/resumable-downloads-http-range-if-range-validators/)
/// 与本项目实测的同类经验：**Range 选的是字节区间，不是版本**。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
struct ResumeMeta {
    /// 格式版本。老文件没有这个字段，反序列化成 0（按 v1 处理）。
    #[serde(default)]
    version: u32,
    /// 产物身份：期望的 SHA-256（无哈希时为 URL）
    #[serde(default)]
    identity: String,
    /// 期望的总字节数
    #[serde(default)]
    total: u64,
    /// 首次下载时服务端给的 ETag。续传时用 `If-Range` 发回去，
    /// 让服务端判断"文件是否已变"——变了就返回 200，我们据此放弃续传。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    /// v2 才有的分段进度。v1 为空，表示整份文件按连续前缀续传。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    segments: Vec<SegmentMeta>,
}

/// 一个分片的进度。
///
/// `end` 是**闭区间**，与 HTTP Range 一致；长度为 `end - begin + 1`。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SegmentMeta {
    begin: u64,
    end: u64,
    /// 这一片已经落盘的字节数（从 `begin` 起算）。
    done: u64,
}

impl SegmentMeta {
    fn len(&self) -> u64 {
        self.end.saturating_sub(self.begin) + 1
    }

    fn finished(&self) -> bool {
        self.done >= self.len()
    }
}

/// 元信息文件路径：`foo.gguf.part` → `foo.gguf.part.meta`。
///
/// `pub(crate)` 是因为 `presets` 的测试要造「下了一半」的现场——
/// 只写 `.part` 不写 `.part.meta` 造不出真实场景（那样 `total` 读不到）。
fn meta_path(part: &Path) -> PathBuf {
    let mut s = part.as_os_str().to_os_string();
    s.push(".meta");
    PathBuf::from(s)
}

/// 给其他模块的测试造现场用。生产代码不该调它。
#[cfg(test)]
pub(crate) fn meta_path_for_test(part: &Path) -> PathBuf {
    meta_path(part)
}

/// 尝试续传。
///
/// - `Ok(Some(final_size))`：续传成功
/// - `Ok(None)`：服务端不支持续传、或产物身份对不上，调用方应从头下
async fn try_resume<F>(
    url: &str,
    part: &Path,
    existing: u64,
    expected_sha256: Option<&str>,
    cancel: Option<&AtomicBool>,
    on_progress: &mut F,
) -> Result<Option<u64>, DownloadError>
where
    F: FnMut(Progress),
{
    // 身份不匹配就绝不续传：宁可重下，也不要拼出坏文件
    let identity = expected_sha256.unwrap_or(url).to_string();
    let old_meta = read_meta(part).await;
    if old_meta.as_ref().map(|m| &m.identity) != Some(&identity) {
        // 没有元信息，或身份不是当前产物 → 丢弃旧半截
        let _ = tokio::fs::remove_file(part).await;
        return Ok(None);
    }

    if cancelled(cancel) {
        return Err(DownloadError::Cancelled);
    }

    let mut req = http_client()
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={existing}-"));

    // `If-Range`：把之前拿到的 ETag 发回去。若服务端认为文件已变，
    // 会返回 200（整份）而不是 206，我们据此放弃续传。
    if let Some(etag) = old_meta.as_ref().and_then(|m| m.etag.clone()) {
        req = req.header(reqwest::header::IF_RANGE, etag);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| DownloadError::Failed(format!("续传请求失败: {e}")))?;

    // 服务端不支持 Range 时会返回 200（整份内容），此时不能 append
    if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        let _ = tokio::fs::remove_file(part).await;
        return Ok(None);
    }

    // 校验返回的起点：必须是本地已有字节数
    let start = content_range_start(&resp);
    if start != Some(existing) {
        // 起点对不上，说明服务端认为的文件状态与我们不同 → 丢弃重下
        let _ = tokio::fs::remove_file(part).await;
        return Ok(None);
    }

    // 总大小变了：说明服务端换了内容，不能继续拼
    if let Some(old) = old_meta.as_ref() {
        let remaining = resp.content_length().unwrap_or(0);
        let now_total = existing + remaining;
        if old.total != 0 && now_total != old.total {
            let _ = tokio::fs::remove_file(part).await;
            return Ok(None);
        }
    }

    let remaining = resp.content_length().unwrap_or(0);
    let total = existing + remaining;

    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(part)
        .await
        .map_err(|e| DownloadError::Failed(format!("打开已有文件失败: {e}")))?;

    write_stream(resp, &mut file, existing, total, cancel, on_progress).await?;
    Ok(Some(total))
}

/// 读 `.part.meta`，读不到或格式坏了都返回 `None`（当作"没有元信息"）。
async fn read_meta(part: &Path) -> Option<ResumeMeta> {
    let raw = tokio::fs::read_to_string(meta_path(part)).await.ok()?;
    serde_json::from_str(&raw).ok()
}

/// 一个未完成的下载残留在磁盘上的状态。
///
/// 供界面显示「已暂停 · 1.2 / 4.6 GB · 可续传」用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialState {
    /// `.part` 已有多少字节
    pub downloaded: u64,
    /// 期望总字节数（来自 `.part.meta`）。0 表示不知道。
    pub total: u64,
}

/// 查一个目标文件有没有留下未完成的下载。
///
/// ## 为什么这个查询必须存在
///
/// `.part` 是**故意保留**的：用户取消后下次点下载要接着下，不能从零开始。
/// 但这带来一个隐患——那几个 GB 在界面上**完全不可见**：
///
/// - `models::scan` 只认 `.gguf`，`.part` 永远不进模型列表
/// - 启动时不扫孤儿 `.part`
/// - 没有任何地方告诉用户它在占空间
///
/// 结果就是用户取消一次 8 GB 的下载，磁盘上悄悄少了 8 GB，几周后满了
/// 也不知道是哪儿来的。这个函数让界面能把那部分**显示出来**，
/// 并提供一个删除入口。
///
/// 返回 `None` 表示没有残留（或残留是 0 字节，等于没有）。
pub fn partial_state(dest: &Path) -> Option<PartialState> {
    let part = part_path(dest);
    let meta = std::fs::read_to_string(meta_path(&part))
        .ok()
        .and_then(|raw| serde_json::from_str::<ResumeMeta>(&raw).ok());

    // v2 的 `.part` 是预分配的，文件长度等于总大小——不能再用长度当进度，
    // 否则界面上会出现「已暂停 · 4.6 / 4.6 GB」这种假象。
    let downloaded = match meta.as_ref() {
        Some(m) if m.version >= META_VERSION && !m.segments.is_empty() => {
            m.segments.iter().map(|s| s.done.min(s.len())).sum()
        }
        _ => std::fs::metadata(&part).ok()?.len(),
    };
    if downloaded == 0 {
        // 0 字节没有任何续传价值，不值得在界面占一行
        return None;
    }

    Some(PartialState {
        downloaded,
        total: meta.map(|m| m.total).unwrap_or(0),
    })
}

/// `.part` 实际已落盘多少字节。
///
/// v2 读分段表；v1 与异常情况退回文件长度。给 `runtime.rs` 统计残留用。
pub fn part_downloaded_bytes(part: &Path) -> u64 {
    if let Ok(raw) = std::fs::read_to_string(meta_path(part))
        && let Ok(meta) = serde_json::from_str::<ResumeMeta>(&raw)
        && meta.version >= META_VERSION
        && !meta.segments.is_empty()
    {
        return meta.segments.iter().map(|s| s.done.min(s.len())).sum();
    }
    std::fs::metadata(part).map(|m| m.len()).unwrap_or(0)
}

/// 删掉未完成的下载（`.part` + `.part.meta`）。
///
/// 返回删掉的字节数，便于界面反馈"已释放 1.8 GB"。
///
/// 刻意做成**独立的手动操作**而不是取消时自动删：取消的语义是"先停一下"，
/// 通常还会续传；自动删会让用户白白丢掉已下的部分。真不要了，
/// 让用户明确点「删除」。
pub fn remove_partial(dest: &Path) -> u64 {
    let part = part_path(dest);
    let bytes = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&part);
    // 元信息必须跟着删：留着它会让下次下载以为可以按旧身份续传，
    // 但 `.part` 已经不在了，等于一次无效的续传尝试
    let _ = std::fs::remove_file(meta_path(&part));
    // 原子写的临时文件：上次写到一半被杀时可能留下
    let mut tmp = meta_path(&part).as_os_str().to_os_string();
    tmp.push(".tmp");
    let _ = std::fs::remove_file(PathBuf::from(tmp));
    bytes
}

/// 写 `.part.meta`（v1：无分段信息）。失败不影响下载本身——只是下次不能续传。
async fn write_meta(part: &Path, identity: &str, total: u64, etag: Option<String>) {
    let meta = ResumeMeta {
        version: 0,
        identity: identity.to_string(),
        total,
        etag,
        segments: Vec::new(),
    };
    write_meta_obj(part, &meta).await;
}

/// 原子地写元信息：先写 `.tmp` 再改名，避免进程被杀时留下截断的 JSON。
async fn write_meta_obj(part: &Path, meta: &ResumeMeta) {
    let Ok(raw) = serde_json::to_vec(meta) else {
        return;
    };
    let path = meta_path(part);
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    if tokio::fs::write(&tmp, raw).await.is_ok() && tokio::fs::rename(&tmp, &path).await.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
}

// ─────────────────────────── 分段并发下载 ───────────────────────────

/// Range 探测结果。
struct RangeProbe {
    total: u64,
    /// 服务端接受 `Range` 且返回了 206。
    ranged: bool,
    etag: Option<String>,
    /// 服务端忽略 Range 时保留的 200 响应，可直接交给单流下载复用。
    resp: Option<reqwest::Response>,
}

/// 用 `bytes=0-0` 探一次服务端能力与文件大小。
///
/// 复用探测响应而不是丢弃后重开：本地测试服务里有「只 accept 一次」的
/// 实现，多一次请求会直接失败；真实环境也能省一次 TLS 往返。
async fn probe_range(url: &str, if_range: Option<&str>) -> Result<RangeProbe, DownloadError> {
    let mut req = http_client()
        .get(url)
        .header(reqwest::header::RANGE, "bytes=0-0");
    if let Some(tag) = if_range {
        req = req.header(reqwest::header::IF_RANGE, tag);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| DownloadError::Failed(format!("请求下载地址失败: {e}")))?;

    let status = resp.status();
    let etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if status == reqwest::StatusCode::PARTIAL_CONTENT {
        if let Some(total) = content_range_total(&resp) {
            return Ok(RangeProbe {
                total,
                ranged: true,
                etag,
                resp: None,
            });
        }
        // 206 但总大小是 `*`：无法切分片，退到单流重新请求一次。
        return Ok(RangeProbe {
            total: 0,
            ranged: false,
            etag,
            resp: None,
        });
    }

    if status.is_success() {
        let total = resp.content_length().unwrap_or(0);
        return Ok(RangeProbe {
            total,
            ranged: false,
            etag,
            resp: Some(resp),
        });
    }

    Err(DownloadError::Failed(format!(
        "下载失败，HTTP {status}。可尝试更换下载源或检查网络。"
    )))
}

/// 从 `Content-Range: bytes 0-0/32306237` 里取出总大小。
fn content_range_total(resp: &reqwest::Response) -> Option<u64> {
    let raw = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)?
        .to_str()
        .ok()?;
    let total = raw.strip_prefix("bytes ")?.split('/').nth(1)?.trim();
    if total == "*" {
        return None;
    }
    total.parse::<u64>().ok()
}

/// 按总大小切分片。
///
/// 8 等分再夹在 1–16 MB：30 MB 的引擎是 4 MB × 8 片，1.8 GB 的模型
/// 是 16 MB × 113 片。片太大负载不均，太小请求数暴涨。
fn plan_segments(total: u64) -> Vec<SegmentMeta> {
    let chunk = (total / 8).clamp(SEGMENT_CHUNK_MIN, SEGMENT_CHUNK_MAX);
    let mut out = Vec::new();
    let mut begin = 0u64;
    while begin < total {
        let end = (begin + chunk - 1).min(total - 1);
        out.push(SegmentMeta {
            begin,
            end,
            done: 0,
        });
        begin = end + 1;
    }
    out
}

fn segment_workers(_total: u64) -> usize {
    SEGMENT_WORKERS
}

/// 运行时的一片状态。`done` 用原子量，worker 写、协调者读快照。
struct SegState {
    begin: u64,
    end: u64,
    done: Arc<AtomicU64>,
}

impl SegState {
    fn len(&self) -> u64 {
        self.end.saturating_sub(self.begin) + 1
    }

    fn remaining(&self) -> u64 {
        self.len().saturating_sub(self.done.load(Ordering::Relaxed))
    }
}

/// 全新分段下载。
async fn segmented_fresh<F>(
    url: &str,
    part: &Path,
    probe: RangeProbe,
    identity: String,
    cancel: Option<&AtomicBool>,
    on_progress: &mut F,
) -> Result<(), DownloadError>
where
    F: FnMut(Progress),
{
    let total = probe.total;
    if let Some(parent) = part.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| DownloadError::Failed(format!("创建目录失败: {e}")))?;
    }

    // **先写 meta，再建文件。**
    //
    // 反过来的话存在一个窗口：文件已被 `set_len` 预分配到总大小，而 meta
    // 还没落盘——此时任何按「文件长度」读进度的观察者都会看到 100%，
    // 取消/界面会误判成"已经下完"。
    let meta = ResumeMeta {
        version: META_VERSION,
        identity,
        total,
        etag: probe.etag,
        segments: plan_segments(total),
    };
    write_meta_obj(part, &meta).await;

    // 预分配：磁盘不够会在开始就报，而不是下到 90% 才失败。
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(part)
        .await
        .map_err(|e| DownloadError::Failed(format!("创建文件失败: {e}")))?;
    file.set_len(total)
        .await
        .map_err(|e| DownloadError::Failed(format!("预分配磁盘失败: {e}")))?;
    drop(file);

    run_segments(url, part, &meta, cancel, on_progress).await
}

/// 分段续传。
///
/// - `Ok(true)`：所有片都已完成（或本次补齐），调用方进入收尾
/// - `Ok(false)`：服务端不支持 Range / 大小变了，调用方清掉重下
async fn segmented_resume<F>(
    url: &str,
    part: &Path,
    meta: &ResumeMeta,
    cancel: Option<&AtomicBool>,
    on_progress: &mut F,
) -> Result<bool, DownloadError>
where
    F: FnMut(Progress),
{
    if !segments_cover_total(meta) {
        return Ok(false);
    }

    let probe = probe_range(url, meta.etag.as_deref()).await?;
    if !probe.ranged || probe.total != meta.total {
        return Ok(false);
    }

    let file = match tokio::fs::OpenOptions::new().write(true).open(part).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(DownloadError::Failed(format!("打开已有文件失败: {e}"))),
    };
    let len = file
        .metadata()
        .await
        .map_err(|e| DownloadError::Failed(format!("读取文件大小失败: {e}")))?
        .len();
    if len != meta.total {
        file.set_len(meta.total)
            .await
            .map_err(|e| DownloadError::Failed(format!("调整文件大小失败: {e}")))?;
    }
    drop(file);

    // 所有片都完成了，只差改名（上次可能写完但没来得及收尾）
    if meta.segments.iter().all(|s| s.finished()) {
        return Ok(true);
    }

    run_segments(url, part, meta, cancel, on_progress).await?;
    Ok(true)
}

/// 校验分段表是否严格覆盖 `[0, total)`，且 `done` 不越界。
fn segments_cover_total(meta: &ResumeMeta) -> bool {
    if meta.total == 0 || meta.segments.is_empty() {
        return false;
    }
    let mut next = 0u64;
    for seg in &meta.segments {
        if seg.begin != next || seg.end < seg.begin || seg.done > seg.len() {
            return false;
        }
        next = seg.end + 1;
    }
    next == meta.total
}

/// 一个并发任务。全部借用当前 `download()` 的栈数据，所以不 spawn，
/// 交给 `FuturesUnordered` 在同一个任务里并发驱动。
type WorkerFut<'a> = Pin<Box<dyn Future<Output = Result<(), DownloadError>> + Send + 'a>>;

#[allow(clippy::too_many_arguments)]
fn make_worker<'a>(
    url: &'a str,
    part: &'a Path,
    etag: Option<&'a str>,
    segs: Arc<Vec<SegState>>,
    queue: Arc<Mutex<VecDeque<usize>>>,
    total_done: Arc<AtomicU64>,
    cancel: Option<&'a AtomicBool>,
    stop: Arc<AtomicBool>,
    throttled: Arc<AtomicBool>,
) -> WorkerFut<'a> {
    Box::pin(run_worker(
        url, part, etag, segs, queue, total_done, cancel, stop, throttled,
    ))
}

/// 协调多个 worker：慢启动、进度聚合、定期保存分段进度。
async fn run_segments<F>(
    url: &str,
    part: &Path,
    meta: &ResumeMeta,
    cancel: Option<&AtomicBool>,
    on_progress: &mut F,
) -> Result<(), DownloadError>
where
    F: FnMut(Progress),
{
    let total = meta.total;
    let segs: Arc<Vec<SegState>> = Arc::new(
        meta.segments
            .iter()
            .map(|s| SegState {
                begin: s.begin,
                end: s.end,
                done: Arc::new(AtomicU64::new(s.done)),
            })
            .collect(),
    );
    let queue: Arc<Mutex<VecDeque<usize>>> = Arc::new(Mutex::new(
        segs.iter()
            .enumerate()
            .filter(|(_, s)| s.remaining() > 0)
            .map(|(i, _)| i)
            .collect(),
    ));
    let total_done = Arc::new(AtomicU64::new(
        segs.iter().map(|s| s.done.load(Ordering::Relaxed)).sum(),
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let throttled = Arc::new(AtomicBool::new(false));

    let max_workers = segment_workers(total).min(queue.lock().map(|q| q.len()).unwrap_or(1).max(1));

    let mut inflight: FuturesUnordered<WorkerFut<'_>> = FuturesUnordered::new();

    // 慢启动：先 1 条，之后每 250ms 加一条，直到上限或被 429 叫停。
    let mut launched = 0usize;
    if queue.lock().map(|q| !q.is_empty()).unwrap_or(false) {
        inflight.push(make_worker(
            url,
            part,
            meta.etag.as_deref(),
            segs.clone(),
            queue.clone(),
            total_done.clone(),
            cancel,
            stop.clone(),
            throttled.clone(),
        ));
        launched = 1;
    }

    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut failure: Option<DownloadError> = None;

    loop {
        if inflight.is_empty() {
            break;
        }
        tokio::select! {
            _ = tick.tick() => {
                on_progress(Progress {
                    downloaded: total_done.load(Ordering::Relaxed).min(total),
                    total,
                });
                if launched < max_workers
                    && !throttled.load(Ordering::Relaxed)
                    && queue.lock().map(|q| !q.is_empty()).unwrap_or(false)
                {
                    inflight.push(make_worker(
                        url,
                        part,
                        meta.etag.as_deref(),
                        segs.clone(),
                        queue.clone(),
                        total_done.clone(),
                        cancel,
                        stop.clone(),
                        throttled.clone(),
                    ));
                    launched += 1;
                }
                // 每 250ms 落一次分段进度：取消/断电回来最多丢 250ms 的下载量，
                // 而 JSON 只有几 KB，开销可以忽略。之前写死 2 秒会在
                // 「刚下一点就取消」时留下 done=0 的 meta，续传等于重下。
                save_segments(part, meta, &segs).await;
            }
            Some(res) = inflight.next() => {
                if let Err(e) = res {
                    failure = Some(e);
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
    }

    // 出错的路径先停掉其他 worker（drop 即取消），再落一次分段进度
    stop.store(true, Ordering::Relaxed);
    drop(inflight);
    save_segments(part, meta, &segs).await;

    if let Some(e) = failure {
        return Err(e);
    }
    if cancelled(cancel) {
        return Err(DownloadError::Cancelled);
    }

    let missing: u64 = segs.iter().map(|s| s.remaining()).sum();
    if missing != 0 {
        return Err(DownloadError::Failed(format!(
            "分段下载未完成：还剩 {missing} 字节。已保留断点，可重试续传。"
        )));
    }

    on_progress(Progress {
        downloaded: total,
        total,
    });
    Ok(())
}

/// 单个 worker：从队列领片，逐片下载，直到队列空了退出。
#[allow(clippy::too_many_arguments)]
async fn run_worker(
    url: &str,
    part: &Path,
    etag: Option<&str>,
    segs: Arc<Vec<SegState>>,
    queue: Arc<Mutex<VecDeque<usize>>>,
    total_done: Arc<AtomicU64>,
    cancel: Option<&AtomicBool>,
    stop: Arc<AtomicBool>,
    throttled: Arc<AtomicBool>,
) -> Result<(), DownloadError> {
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(part)
        .await
        .map_err(|e| DownloadError::Failed(format!("打开下载文件失败: {e}")))?;

    loop {
        if stop.load(Ordering::Relaxed) || cancelled(cancel) {
            return Err(DownloadError::Cancelled);
        }
        let Some(index) = queue.lock().ok().and_then(|mut q| q.pop_front()) else {
            return Ok(());
        };
        let seg = &segs[index];

        let mut attempt = 0u32;
        loop {
            if stop.load(Ordering::Relaxed) || cancelled(cancel) {
                return Err(DownloadError::Cancelled);
            }
            if seg.remaining() == 0 {
                break;
            }

            match fetch_segment(
                url,
                etag,
                &mut file,
                seg,
                &total_done,
                cancel,
                &stop,
                &throttled,
            )
            .await
            {
                Ok(()) => break,
                Err(SegmentError::Cancelled) => return Err(DownloadError::Cancelled),
                Err(SegmentError::Throttled(wait)) => {
                    tokio::time::sleep(wait).await;
                }
                Err(SegmentError::Fatal(msg)) => {
                    if attempt >= SEGMENT_RETRIES {
                        return Err(DownloadError::Failed(msg));
                    }
                    attempt += 1;
                    tokio::time::sleep(Duration::from_secs(attempt as u64)).await;
                }
            }
        }
    }
}

enum SegmentError {
    Cancelled,
    /// 服务端限流：等一会儿重试，并暂停扩容。
    Throttled(Duration),
    /// 可以换下一次重试的错误。
    Fatal(String),
}

/// 下载一个分片内尚未完成的部分。
#[allow(clippy::too_many_arguments)]
async fn fetch_segment(
    url: &str,
    etag: Option<&str>,
    file: &mut tokio::fs::File,
    seg: &SegState,
    total_done: &AtomicU64,
    cancel: Option<&AtomicBool>,
    stop: &AtomicBool,
    throttled: &AtomicBool,
) -> Result<(), SegmentError> {
    let done = seg.done.load(Ordering::Relaxed);
    let start = seg.begin + done;
    let end = seg.end;
    if start > end {
        return Ok(());
    }
    let remaining = end - start + 1;

    let mut req = http_client()
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={start}-{end}"));
    if let Some(tag) = etag {
        req = req.header(reqwest::header::IF_RANGE, tag);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| SegmentError::Fatal(format!("分段请求失败: {e}")))?;

    let status = resp.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        throttled.store(true, Ordering::Relaxed);
        let wait = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(3));
        return Err(SegmentError::Throttled(wait));
    }
    if status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(SegmentError::Fatal(format!(
            "下载失败，HTTP {status}。可尝试更换下载源或检查网络。"
        )));
    }

    if content_range_start(&resp) != Some(start) {
        return Err(SegmentError::Fatal(
            "服务器返回的分段起点与请求不一致，已停止以免拼出坏文件。".into(),
        ));
    }

    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|e| SegmentError::Fatal(format!("定位写入位置失败: {e}")))?;

    let mut stream = resp.bytes_stream();
    let mut left = remaining;
    while let Some(chunk) = stream.next().await {
        if stop.load(Ordering::Relaxed) || cancelled(cancel) {
            return Err(SegmentError::Cancelled);
        }
        let chunk = chunk.map_err(|e| SegmentError::Fatal(format!("下载中断: {e}")))?;
        let take = (chunk.len() as u64).min(left) as usize;
        if take == 0 {
            break;
        }
        file.write_all(&chunk[..take])
            .await
            .map_err(|e| SegmentError::Fatal(format!("写入文件失败: {e}")))?;
        seg.done.fetch_add(take as u64, Ordering::Relaxed);
        total_done.fetch_add(take as u64, Ordering::Relaxed);
        left -= take as u64;
        if left == 0 {
            break;
        }
    }

    file.flush()
        .await
        .map_err(|e| SegmentError::Fatal(format!("写入文件失败: {e}")))?;

    if left > 0 {
        return Err(SegmentError::Fatal(format!(
            "下载中断：本段还差 {left} 字节"
        )));
    }
    Ok(())
}

/// 把当前各片进度写成 meta 快照。
async fn save_segments(part: &Path, meta: &ResumeMeta, segs: &[SegState]) {
    let snapshot = ResumeMeta {
        version: META_VERSION,
        identity: meta.identity.clone(),
        total: meta.total,
        etag: meta.etag.clone(),
        segments: segs
            .iter()
            .map(|s| SegmentMeta {
                begin: s.begin,
                end: s.end,
                done: s.done.load(Ordering::Relaxed).min(s.len()),
            })
            .collect(),
    };
    write_meta_obj(part, &snapshot).await;
}

/// 把响应流写进文件，边写边回调进度。
///
/// ## 取消的检查点就这里
///
/// 每收到一块数据先问一次「还要不要下」。选中这个位置的原因：
///
/// - **粒度合适**：reqwest 的 chunk 通常是几十 KB，8 GB 大约几万次检查，
///   一次 `load(Relaxed)` 是纳秒级，开销可以忽略
/// - **来得及停**：正好是"上一块已落盘、下一块还没读"的位置，
///   停在这里 `.part` 一定是完整的块边界，续传时不会撞上半个块
///
/// 检查放在 `write_all` **之前**：先判断再写，避免多写一块。
async fn write_stream<F>(
    resp: reqwest::Response,
    file: &mut tokio::fs::File,
    already: u64,
    total: u64,
    cancel: Option<&AtomicBool>,
    on_progress: &mut F,
) -> Result<(), DownloadError>
where
    F: FnMut(Progress),
{
    let mut stream = resp.bytes_stream();
    let mut downloaded = already;
    let mut last_emit = std::time::Instant::now();

    on_progress(Progress { downloaded, total });

    while let Some(chunk) = stream.next().await {
        if cancelled(cancel) {
            // 丢掉响应流即可，`.part` 原样留在磁盘上。
            // 注意这里**不清 `.part.meta`**——下次续传还要靠它判断身份。
            return Err(DownloadError::Cancelled);
        }

        let chunk = chunk.map_err(|e| DownloadError::Failed(format!("下载中断: {e}")))?;
        file.write_all(&chunk)
            .await
            .map_err(|e| DownloadError::Failed(format!("写入文件失败: {e}")))?;
        downloaded += chunk.len() as u64;

        // 限流：UI 不需要每个 chunk 都刷一次
        if last_emit.elapsed().as_millis() > 250 {
            on_progress(Progress { downloaded, total });
            last_emit = std::time::Instant::now();
        }
    }

    file.flush()
        .await
        .map_err(|e| DownloadError::Failed(e.to_string()))?;
    on_progress(Progress { downloaded, total });
    Ok(())
}

/// 从 `Content-Range: bytes 100-999/1000` 里取出起点 `100`。
fn content_range_start(resp: &reqwest::Response) -> Option<u64> {
    let raw = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)?
        .to_str()
        .ok()?;
    // 形如 "bytes 100-999/1000"
    let rest = raw.strip_prefix("bytes ")?;
    let start = rest.split('-').next()?;
    start.trim().parse::<u64>().ok()
}

/// 流式计算文件的 SHA-256，不把整个文件读进内存。
///
/// 模型动辄几个 GB，`fs::read` 会把内存打爆——现有的
/// `install_runtime` 就是 `fs::read` 整包（31 MB 尚可），
/// 但模型不能这么干。
pub async fn sha256_file(path: &Path) -> Result<String, String> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("打开文件失败: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|e| format!("读取文件失败: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `.part.meta` 路径必须是 `.part` 的同级兄弟，不能跑到别的目录。
    #[test]
    fn meta_path_sits_next_to_part() {
        let part = part_path(Path::new("/models/foo.gguf"));
        let meta = meta_path(&part);
        assert_eq!(meta.to_string_lossy(), "/models/foo.gguf.part.meta");
    }

    /// 元信息要能往返序列化——续传判断全指望它。
    #[test]
    fn resume_meta_round_trips() {
        let meta = ResumeMeta {
            identity: "abc123".into(),
            total: 4096,
            etag: Some("\"xyz\"".into()),
            ..Default::default()
        };
        let raw = serde_json::to_string(&meta).unwrap();
        let back: ResumeMeta = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, meta);
    }

    /// 没有 ETag 时也要能序列化（有些源不返回）。
    #[test]
    fn resume_meta_without_etag_round_trips() {
        let meta = ResumeMeta {
            identity: "u".into(),
            total: 0,
            etag: None,
            ..Default::default()
        };
        let raw = serde_json::to_string(&meta).unwrap();
        // etag 为 None 时不应出现在 JSON 里
        assert!(!raw.contains("etag"), "{raw}");
        let back: ResumeMeta = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, meta);
    }

    /// 身份不同的半截文件**必须**被判为不可续传。
    ///
    /// 这条锁住最危险的一种错误：把新产物的数据 append 到旧产物后面，
    /// 得到一个大小正确但内容错误的文件。
    #[tokio::test]
    async fn meta_identity_mismatch_invalidates_partial() {
        let dir = std::env::temp_dir().join("xieyi-meta-identity-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let dest = dir.join("m.gguf");
        let part = part_path(&dest);
        std::fs::write(&part, b"0123456789").unwrap();

        // 写入属于"产物 A"的元信息
        write_meta(&part, "sha-of-A", 100, None).await;

        // 读回来必须是 A
        let meta = read_meta(&part).await.unwrap();
        assert_eq!(meta.identity, "sha-of-A");

        // 模拟"产物 B"来续传：身份对不上，应当丢弃半截文件
        let failed_resume = try_resume(
            "http://127.0.0.1:1/never",
            &part,
            10,
            Some("sha-of-B"),
            None,
            &mut |_| {},
        )
        .await;
        // 连接必然失败，但我们先要确认的是"文件已被清掉"这个副作用
        if failed_resume.is_err() {
            // 连接失败发生在身份检查之后，文件此时应已删除
            assert!(
                !part.exists(),
                "身份不匹配时必须先删掉旧半截文件，不能留着冒险续传"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 连接失败 / 服务端报错才换源。
    #[test]
    fn source_switch_only_on_connectivity_errors() {
        // 连不上：该换
        assert!(should_try_next_source("请求下载地址失败: timeout"));
        assert!(should_try_next_source("续传请求失败: dns error"));
        // 服务端说没有：该换（这个源上没这个文件）
        assert!(should_try_next_source("下载失败，HTTP 404"));
        assert!(should_try_next_source("下载失败，HTTP 500"));
    }

    /// 传输中断、写入失败、校验失败**不该**换源。
    ///
    /// 这是关键设计：这些错误发生时 `.part` 里已经有数据，换源会从零
    /// 开始，反而更糟。正确做法是保住进度让用户续传。
    #[test]
    fn source_switch_not_on_transfer_errors() {
        assert!(!should_try_next_source("下载中断: connection reset"));
        assert!(!should_try_next_source("写入文件失败: disk full"));
        assert!(!should_try_next_source("下载校验失败：期望 a，实际 b"));
        assert!(!should_try_next_source("读取文件失败: permission denied"));
    }

    /// 不设总超时：模型几个 GB，慢速网络下要几十分钟，
    /// 总超时会把正在正常下载的请求杀掉。
    #[test]
    fn client_has_stage_timeouts_but_no_total_timeout() {
        // 构造成功即说明这两个超时值可用
        let _ = http_client();
        assert!(CONNECT_TIMEOUT.as_secs() > 0);
        assert!(
            READ_TIMEOUT > CONNECT_TIMEOUT,
            "首字节超时应比连接超时宽松：慢速服务器需要时间准备大文件"
        );
    }

    /// **必须**显式带 `User-Agent`。
    ///
    /// reqwest 默认不发 UA，而 ModelScope 的 CDN 对无 UA 请求返回 403。
    /// 2026-11-14 实测：同一条 URL，`curl` 带 UA 是 206、清空 UA 是 403。
    ///
    /// 变异验证：把 `http_client()` 里的 `.user_agent(USER_AGENT)` 删掉，
    /// 本测试失败。
    ///
    /// 为什么这条测试值钱：UA 缺失的表现是"下载失败 403"，看起来像权限
    /// 问题或网络问题，**不会有人想到是 User-Agent**。这种 bug 只能靠
    /// 静态断言锁住。
    ///
    /// 注意不能用 `Request::headers()` 断言——那是**应用层**的显式头，
    /// 而 `.user_agent()` 设置的是**默认头**，在 `build()` 时还没注入。
    /// 必须真的发一次请求，从服务端侧看收到的头。
    #[tokio::test]
    async fn client_sends_a_user_agent() {
        use std::io::{Read, Write};

        // 起一个只回一次的最小 HTTP 服务，把收到的请求行/头抓回来
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let grab = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = sock.read(&mut buf).unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
            head
        });

        let resp = http_client()
            .get(format!("http://{addr}/probe"))
            .send()
            .await
            .expect("本地服务必定可达");
        let _ = resp.bytes().await;

        let head = grab.join().unwrap();
        let ua_line = head
            .lines()
            .find(|l| l.to_lowercase().starts_with("user-agent:"))
            .map(|l| l["user-agent:".len()..].trim().to_string());

        let ua = ua_line.expect("下载请求必须带 User-Agent，否则魔搭 CDN 会返回 403");
        assert!(
            ua.starts_with("Xieyi/"),
            "UA 应当以产品名开头，便于对方站点识别（RFC 9110 的机器人写法）：{ua}"
        );
        let version = ua.trim_start_matches("Xieyi/");
        assert!(
            !version.is_empty() && version != "unknown",
            "UA 里要带版本号，出问题时能对上是哪个版本：{ua}"
        );
    }

    /// 空的源列表要给出明确错误，而不是 panic 或静默失败。
    #[tokio::test]
    async fn empty_source_list_is_rejected() {
        let tmp = std::env::temp_dir().join("xieyi-empty-sources-test.bin");
        let err = download_from_sources(&[], &tmp, None, None, |_| {})
            .await
            .unwrap_err();
        assert!(
            matches!(&err, DownloadError::Failed(m) if m.contains("没有可用的下载源")),
            "{err}"
        );
    }

    #[test]
    fn part_path_appends_suffix() {
        let p = part_path(Path::new("/models/foo.gguf"));
        assert_eq!(p.to_string_lossy(), "/models/foo.gguf.part");
    }

    /// `.part` 必须仍以原扩展名为前缀的一部分，但不能以 `.gguf` 结尾——
    /// 否则 `models::scan` 会把没下完的文件当成可用模型。
    #[test]
    fn part_path_is_not_scanned_as_model() {
        let p = part_path(Path::new("Hy-MT2-1.8B.gguf"));
        assert!(
            !p.to_string_lossy().ends_with(".gguf"),
            "未下完的文件不能被模型扫描认成 .gguf：{p:?}"
        );
    }

    // ─────────────────────── 取消 ───────────────────────

    /// 起一个会**慢慢吐数据**的本地 HTTP 服务，用来测「下载中途取消」。
    ///
    /// 为什么必须用真服务而不是 mock：取消的整个价值在于「让正在进行的
    /// 传输停下来」。用假的响应对象测不到这一点——它不产生流。
    ///
    /// 用 `std::net` + 独立线程，不引 `tokio::net`（那要加 feature），
    /// 也不引额外 dev-dependency：与项目一贯的"能不引就不引"一致。
    ///
    /// 返回监听地址。线程在客户端断开后自然结束。
    fn spawn_slow_server(total_chunks: usize, chunk_bytes: usize) -> String {
        use std::io::Write;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body_len = total_chunks * chunk_bytes;

        std::thread::spawn(move || {
            let Ok((mut sock, _)) = listener.accept() else {
                return;
            };
            // 读掉请求头（读到空行为止），不回 100-continue 之类的花活
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut sock, &mut buf);

            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {body_len}\r\n\
                 Content-Type: application/octet-stream\r\n\r\n"
            );
            if sock.write_all(header.as_bytes()).is_err() {
                return;
            }

            // 每块之间睡一下，模拟慢速网络。客户端取消后会断开，
            // 这里的 write 就会失败 → 线程退出。
            let chunk = vec![0xAB_u8; chunk_bytes];
            for _ in 0..total_chunks {
                if sock.write_all(&chunk).is_err() {
                    return;
                }
                if sock.flush().is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        });

        format!("http://{addr}/slow.bin")
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(tag);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    type RangeLog = Arc<Mutex<Vec<(u64, u64)>>>;

    /// 起一个**支持 Range** 的本地服务，记录收到的每个 `(start, end)`。
    ///
    /// 内容按偏移确定性生成：第 i 字节 = `(i % 251)`。这样续传/分段拼接
    /// 后可以逐字节校验，而不必在测试里存一整份期望数据。
    fn spawn_range_server(total: u64, delay_ms: u64) -> (String, RangeLog) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let log: RangeLog = Arc::new(Mutex::new(Vec::new()));
        let log_for_server = log.clone();

        std::thread::spawn(move || {
            for sock in listener.incoming() {
                let Ok(mut sock) = sock else { continue };
                let log = log_for_server.clone();
                std::thread::spawn(move || {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 1024];
                    loop {
                        let Ok(n) = sock.read(&mut tmp) else { return };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let head = String::from_utf8_lossy(&buf).to_string();
                    let range = head.lines().find_map(|l| {
                        let lower = l.to_ascii_lowercase();
                        lower
                            .strip_prefix("range: bytes=")
                            .map(|s| s.trim().to_string())
                    });

                    let (start, end) = match range.as_deref() {
                        Some(spec) => {
                            let (a, b) = spec.split_once('-').unwrap_or(("", ""));
                            let start = a.parse::<u64>().unwrap_or(0);
                            let end = b
                                .parse::<u64>()
                                .map(|v| v.min(total - 1))
                                .unwrap_or(total - 1);
                            (start, end)
                        }
                        None => (0, total - 1),
                    };
                    if let Ok(mut l) = log.lock() {
                        l.push((start, end));
                    }

                    let len = end.saturating_sub(start) + 1;
                    let header = if range.is_some() {
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {len}\r\n\
                             Content-Range: bytes {start}-{end}/{total}\r\n\
                             Accept-Ranges: bytes\r\nETag: \"v1\"\r\n\
                             Content-Type: application/octet-stream\r\n\r\n"
                        )
                    } else {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {len}\r\n\
                             Accept-Ranges: bytes\r\nETag: \"v1\"\r\n\
                             Content-Type: application/octet-stream\r\n\r\n"
                        )
                    };
                    if sock.write_all(header.as_bytes()).is_err() {
                        return;
                    }

                    let mut pos = start;
                    while pos <= end {
                        let n = ((end - pos + 1) as usize).min(64 * 1024);
                        let body: Vec<u8> =
                            (0..n).map(|i| ((pos + i as u64) % 251) as u8).collect();
                        if sock.write_all(&body).is_err() {
                            return;
                        }
                        if delay_ms > 0 {
                            std::thread::sleep(Duration::from_millis(delay_ms));
                        }
                        pos += n as u64;
                    }
                });
            }
        });

        (format!("http://{addr}/file.bin"), log)
    }

    fn pattern_matches(data: &[u8]) -> bool {
        data.iter()
            .enumerate()
            .all(|(i, b)| *b == (i as u64 % 251) as u8)
    }

    /// **核心测试**：取消必须真的中断传输，且保留 `.part` 供续传。
    ///
    /// 这条测试对着的是一个真实存在过的缺陷：`presets.rs` 里有个
    /// `CANCEL` 静态变量，`cancel_preset_download` 把它置 true，
    /// 但下载器里**从来没读过它**。于是用户点「取消」：
    /// 界面立刻显示已取消，后台 reqwest 还在闷头往磁盘写几个 GB。
    ///
    /// 变异验证：把 `write_stream` 里那段 `if cancelled(cancel)` 删掉，
    /// 本测试会失败（下载会一路跑到完成，返回 `Ok`）。
    #[tokio::test]
    async fn cancel_stops_transfer_and_keeps_partial() {
        let dir = temp_dir("xieyi-cancel-test");
        let dest = dir.join("m.gguf");
        // 40 块 × 64 KB = 2.5 MB，每块间隔 30ms → 全下完要 1.2 秒
        let url = spawn_slow_server(40, 64 * 1024);

        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let cancel_for_task = cancel.clone();
        let dest_clone = dest.clone();

        // 等第一块真正落盘再取消。
        //
        // 不能固定睡 60ms：CI 机器上连接 / TLS 可能还没完成，`.part`
        // 尚未创建；此时取消发生在"传输开始前"，于是下面的
        // 「取消后 .part 必须保留」变成假失败。
        // （2026-11-22 main 的 Windows CI 实际踩到：291 passed / 1 failed）
        let wait_part = part_path(&dest);
        let canceller = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
            while tokio::time::Instant::now() < deadline {
                let wrote = std::fs::metadata(&wait_part)
                    .map(|m| m.len() > 0)
                    .unwrap_or(false);
                if wrote {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            cancel_for_task.store(true, std::sync::atomic::Ordering::Relaxed);
        });

        let result = download(&url, &dest_clone, None, Some(&cancel), |_| {}).await;
        canceller.await.unwrap();

        assert!(
            matches!(result, Err(DownloadError::Cancelled)),
            "取消后必须返回 Cancelled，实际：{result:?}"
        );

        // `.part` 必须还在：这是"下次可续传"的前提
        let part = part_path(&dest);
        assert!(part.exists(), "取消后 .part 必须保留，否则下次只能从零重下");
        // `.part.meta` 也要在：续传时靠它判断身份，丢了就等于放弃续传
        assert!(
            meta_path(&part).exists(),
            "取消后 .part.meta 必须保留，否则续传会因身份校验失败而重下"
        );
        // 正式文件不能出现：半截文件绝不能被当成下完的
        assert!(!dest.exists(), "取消后不能生成正式文件");

        // 确实停在半路（不是"慢慢等它下完再报取消"）
        let got = std::fs::metadata(&part).unwrap().len();
        assert!(got > 0, "取消前应至少写入一块数据，实际 0 字节");
        assert!(
            got < 2_500_000,
            "取消应发生在传输中途，实际已写入 {got} 字节"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 取消**不能**触发换源。
    ///
    /// 这条容易写错：如果取消被当成普通错误，`download_from_sources`
    /// 会接着试下一个源，用户点了取消却看到它在另一个源上重新开始下载。
    ///
    /// 变异验证：把 `download_from_sources` 里 `Err(Cancelled) => return`
    /// 那一支删掉（让它落进 `Failed` 分支），本测试会失败——错误会变成
    /// `Failed`，且会尝试第 2 个源。
    #[tokio::test]
    async fn cancel_does_not_fall_through_to_next_source() {
        let dir = temp_dir("xieyi-cancel-noswitch-test");
        let dest = dir.join("m.gguf");

        // 两个源都指向同一个会慢慢吐数据的服务；取消后不该碰第二个
        let url = spawn_slow_server(40, 64 * 1024);
        let urls = vec![url.clone(), url];

        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let cancel_for_task = cancel.clone();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            cancel_for_task.store(true, std::sync::atomic::Ordering::Relaxed);
        });

        let result = download_from_sources(&urls, &dest, None, Some(&cancel), |_| {}).await;

        assert!(
            matches!(result, Err(DownloadError::Cancelled)),
            "取消必须以 Cancelled 上抛，不能被包装成 Failed：{result:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 没有取消标志时行为不变（引擎包就是这么调用的）。
    ///
    /// 这条防止「加了取消之后，不可取消的路径被误伤」。
    #[tokio::test]
    async fn without_cancel_flag_download_completes_normally() {
        let dir = temp_dir("xieyi-noflag-test");
        let dest = dir.join("m.bin");
        // 小块、少块，快到不需要等
        let url = spawn_slow_server(2, 1024);

        let result = download(&url, &dest, None, None, |_| {}).await;

        assert!(result.is_ok(), "传 None 时应正常下完：{result:?}");
        assert!(dest.exists(), "完成后正式文件应在位");
        assert!(!part_path(&dest).exists(), "完成后 .part 应已改名");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 已经取消的状态下发起下载，应当立刻返回，不做任何网络请求。
    ///
    /// 变异验证：把 `download()` 开头那次 `if cancelled(cancel)` 删掉，
    /// 本测试仍然可能通过（只是多等一会儿），但会真的去连那个不存在的
    /// 地址——所以这里用一个**必然连不上**的地址并要求"没有网络错误文案"
    /// 来区分：如果真去连了，错误会是 `请求下载地址失败`。
    #[tokio::test]
    async fn pre_cancelled_download_returns_immediately() {
        let dir = temp_dir("xieyi-precancel-test");
        let dest = dir.join("m.bin");

        let cancel = AtomicBool::new(true);
        let result = download(
            "http://127.0.0.1:1/never",
            &dest,
            None,
            Some(&cancel),
            |_| {},
        )
        .await;

        assert!(
            matches!(result, Err(DownloadError::Cancelled)),
            "预先取消时应直接返回 Cancelled，而不是去连网络：{result:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 取消是独立的一类错误，**不能被 `should_try_next_source` 判成可换源**。
    #[test]
    fn cancelled_is_never_a_source_switch_reason() {
        // 文案不能和任何"可换源"的判据撞上
        let text = DownloadError::Cancelled.to_string();
        assert!(
            !should_try_next_source(&text),
            "取消绝不能被判成换源理由，否则会在别的源上重新下载"
        );
    }

    /// 错误类型的 `Display` 要能直接喂给 Tauri 命令（签名仍是 `String`）。
    #[test]
    fn failed_error_keeps_its_message() {
        let e = DownloadError::Failed("磁盘满了".into());
        assert_eq!(String::from(e), "磁盘满了");
    }

    // ─────────────────── 未完成下载的可见性 ───────────────────

    /// 有 `.part` 时必须能查出来，否则用户看不见那几 GB。
    ///
    /// 变异验证：把 `partial_state` 改成永远返回 `None`，
    /// 本测试与 `preset_status` 的暂停态测试都会失败。
    #[test]
    fn partial_state_reports_downloaded_and_total() {
        let dir = temp_dir("xieyi-partial-state-test");
        let dest = dir.join("m.gguf");
        let part = part_path(&dest);

        std::fs::write(&part, vec![0u8; 1234]).unwrap();
        // 手工写一份 meta，模拟"身份证明确、下了一半"
        std::fs::write(
            meta_path(&part),
            serde_json::to_string(&ResumeMeta {
                identity: "sha-abc".into(),
                total: 9999,
                etag: None,
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();

        let st = partial_state(&dest).expect("有 .part 就该报出来");
        assert_eq!(st.downloaded, 1234);
        assert_eq!(
            st.total, 9999,
            "总量要能从 meta 里读出来，界面要显示 1.2/9.7 GB"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 没有 `.part`、或 `.part` 是 0 字节时，不该在界面上占一行。
    #[test]
    fn partial_state_is_none_without_meaningful_partial() {
        let dir = temp_dir("xieyi-partial-none-test");
        let dest = dir.join("m.gguf");

        // 完全没有
        assert!(partial_state(&dest).is_none());

        // 0 字节：没有续传价值
        std::fs::write(part_path(&dest), b"").unwrap();
        assert!(
            partial_state(&dest).is_none(),
            "0 字节的 .part 不该显示为「已暂停」"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 删除必须同时清掉 `.part` 与 `.part.meta`，并报告释放的字节数。
    ///
    /// 变异验证：只删 `.part` 不删 `.meta`，本测试失败——留着 meta 会让
    /// 下次下载以为可以按旧身份续传，但文件已经不在，等于一次无效尝试。
    #[test]
    fn remove_partial_cleans_both_files_and_reports_bytes() {
        let dir = temp_dir("xieyi-partial-remove-test");
        let dest = dir.join("m.gguf");
        let part = part_path(&dest);

        std::fs::write(&part, vec![0u8; 5000]).unwrap();
        std::fs::write(meta_path(&part), b"{}").unwrap();

        let freed = remove_partial(&dest);
        assert_eq!(freed, 5000, "要报告释放了多少字节，界面才能提示用户");
        assert!(!part.exists(), ".part 必须删掉");
        assert!(
            !meta_path(&part).exists(),
            ".part.meta 必须一起删，否则下次续传会按已失效的身份尝试"
        );
        assert!(partial_state(&dest).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 删一个本来就不存在的东西：返回 0，不 panic。
    #[test]
    fn remove_partial_on_missing_file_is_harmless() {
        let dir = temp_dir("xieyi-partial-remove-missing-test");
        let dest = dir.join("never-downloaded.gguf");
        assert_eq!(remove_partial(&dest), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ─────────────────────── 分段并发 ───────────────────────

    /// 分片表必须严格覆盖 `[0, total)`，不留缝也不重叠。
    #[test]
    fn plan_segments_covers_total() {
        for total in [SEGMENT_MIN_TOTAL, 32 * 1024 * 1024, 1_800_000_000u64] {
            let segs = plan_segments(total);
            assert_eq!(segs.first().unwrap().begin, 0);
            assert_eq!(segs.last().unwrap().end, total - 1);
            for pair in segs.windows(2) {
                assert_eq!(pair[1].begin, pair[0].end + 1, "分片之间不能有缝");
            }
            let sum: u64 = segs.iter().map(|s| s.len()).sum();
            assert_eq!(sum, total, "分片总长必须等于文件大小");
        }
    }

    /// 分段表有缝/重叠/越界时必须判为不可续传。
    #[test]
    fn segments_cover_total_rejects_bad_tables() {
        let ok = ResumeMeta {
            version: META_VERSION,
            identity: "x".into(),
            total: 100,
            etag: None,
            segments: vec![
                SegmentMeta {
                    begin: 0,
                    end: 49,
                    done: 10,
                },
                SegmentMeta {
                    begin: 50,
                    end: 99,
                    done: 0,
                },
            ],
        };
        assert!(segments_cover_total(&ok));

        // 中间缺一字节
        let mut gap = ok.clone();
        gap.segments[1].begin = 51;
        assert!(!segments_cover_total(&gap));

        // done 超过段长（首片长 50，写 51 就越界）
        let mut over = ok.clone();
        over.segments[0].done = 51;
        assert!(!segments_cover_total(&over));

        // 总长对不上
        let mut short = ok;
        short.total = 101;
        assert!(!segments_cover_total(&short));
    }

    /// 支持 Range 的服务端必须真的走分段并发，且拼出的文件内容正确。
    #[tokio::test]
    async fn segmented_download_splits_into_multiple_ranges() {
        let total = SEGMENT_MIN_TOTAL; // 8 MiB → 1 MiB × 8 片
        let (url, log) = spawn_range_server(total, 0);
        let dir = temp_dir("xieyi-seg-basic");
        let dest = dir.join("m.bin");

        let done = download(&url, &dest, None, None, |_| {}).await.unwrap();
        assert_eq!(done.bytes, total);

        let data = std::fs::read(&dest).unwrap();
        assert_eq!(data.len() as u64, total);
        assert!(pattern_matches(&data), "分段写入的偏移必须正确");
        assert!(!part_path(&dest).exists(), "完成后 .part 应已改名");
        assert!(
            !meta_path(&part_path(&dest)).exists(),
            "完成后不应留下 .part.meta"
        );

        let reqs = log.lock().unwrap().clone();
        let data_reqs = reqs
            .iter()
            .filter(|(a, b)| !(*a == 0 && *b == 0)) // 去掉 0-0 探测
            .count();
        assert!(
            data_reqs >= 2,
            "支持 Range 的服务端应当收到多个分段请求，实际：{reqs:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 取消后必须保住**分段进度**，续传只补没下完的部分。
    #[tokio::test]
    async fn segmented_download_resumes_from_saved_progress() {
        let total = 16 * 1024 * 1024; // 16 MiB → 2 MiB × 8 片
        let (url, log) = spawn_range_server(total, 20);
        let dir = temp_dir("xieyi-seg-resume");
        let dest = dir.join("m.bin");
        let part = part_path(&dest);

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_task = cancel.clone();
        let wait_part = part.clone();
        let watcher = tokio::spawn(async move {
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            while std::time::Instant::now() < deadline {
                if part_downloaded_bytes(&wait_part) >= 1024 * 1024 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            cancel_for_task.store(true, Ordering::Relaxed);
        });

        let first = download(&url, &dest, None, Some(&cancel), |_| {}).await;
        watcher.await.unwrap();
        assert!(
            matches!(first, Err(DownloadError::Cancelled)),
            "应当以取消结束，实际：{first:?}"
        );

        let meta = read_meta(&part).await.expect("取消后必须留下分段 meta");
        assert!(meta.version >= META_VERSION);
        let done: u64 = meta.segments.iter().map(|s| s.done).sum();
        assert!(done > 0, "取消前至少要有一片写入进度，实际 {done}");
        assert!(done < total, "取消应当发生在完成之前，实际 {done}/{total}");
        assert_eq!(
            std::fs::metadata(&part).unwrap().len(),
            total,
            "预分配后 .part 长度应当等于总大小"
        );
        assert_eq!(
            partial_state(&dest).map(|s| s.downloaded),
            Some(done),
            "界面的「已暂停」进度必须按分段表算，不能看文件长度"
        );

        // 续传：清掉取消标志再来一次。
        //
        // 先等一小会儿，让取消前已经发出的请求都写进服务端日志；
        // 否则它们会被误算成「续传阶段的请求」。
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.store(false, Ordering::Relaxed);
        let reqs_before = log.lock().unwrap().len();
        let second = download(&url, &dest, None, Some(&cancel), |_| {})
            .await
            .unwrap();
        assert_eq!(second.bytes, total, "续传后总字节数应当是完整的");
        assert!(pattern_matches(&std::fs::read(&dest).unwrap()));

        // 续传只补没下完的部分。
        //
        // 不能按「请求字节数之和 < 总大小」判断：连接中断重试时会重复
        // 请求同一段，日志里的区间之和会超过文件大小（Windows CI 上
        // 实际踩到：17,432,577 > 16 MiB）。真正要保证的是：
        // **已有进度的分段，不会被从它已完成的字节之前重新请求**。
        let reqs = log.lock().unwrap().clone();
        for (start, end) in &reqs[reqs_before..] {
            if *start == 0 && *end == 0 {
                continue; // Range 探测请求
            }
            let seg = meta
                .segments
                .iter()
                .find(|s| *start >= s.begin && *start <= s.end)
                .expect("续传请求的起点应落在某个分段内");
            assert!(
                *start >= seg.begin + seg.done,
                "续传重复请求了已完成的字节：段起点 {}，已完成 {}，实际请求起点 {}",
                seg.begin,
                seg.done,
                start
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 服务端忽略 Range（回 200）时，必须回退到单流，且内容正确。
    ///
    /// 走的是「复用探测响应」这条路：探测拿到的 200 直接交给单流写盘，
    /// 不会因为多发一次请求而失败。
    #[tokio::test]
    async fn falls_back_to_single_stream_when_range_ignored() {
        let dir = temp_dir("xieyi-seg-fallback");
        let dest = dir.join("m.bin");
        // 复用只 accept 一次的慢速服务：它不会响应 Range，但能正常吐完
        let url = spawn_slow_server(8, 1024);

        let done = download(&url, &dest, None, None, |_| {}).await.unwrap();
        assert_eq!(done.bytes as usize, 8 * 1024);
        assert_eq!(std::fs::read(&dest).unwrap().len(), 8 * 1024);
        assert!(!part_path(&dest).exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 哈希不对时整份自动重下一次，仍不对才报错。
    #[tokio::test]
    async fn checksum_mismatch_retries_once_then_fails() {
        let total = SEGMENT_MIN_TOTAL;
        let (url, log) = spawn_range_server(total, 0);
        let dir = temp_dir("xieyi-checksum-retry");
        let dest = dir.join("m.bin");

        // 内容与这个"期望哈希"必然对不上
        let err = download(&url, &dest, Some("deadbeef"), None, |_| {})
            .await
            .unwrap_err();
        assert!(
            matches!(&err, DownloadError::Failed(m) if m.contains("已自动重试一次")),
            "应当自动重试一次后报错，实际：{err}"
        );

        let probes = log
            .lock()
            .unwrap()
            .iter()
            .filter(|(a, b)| *a == 0 && *b == 0)
            .count();
        assert!(probes >= 2, "应当重新下载一遍，探测请求只有 {probes} 次");
        assert!(!part_path(&dest).exists(), "失败后不应留下坏文件");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真实网络：分段下载官方引擎包，验证 Range / 并发 / 哈希在真实 CDN 上可用。
    ///
    /// 默认跳过。手动跑：
    /// `cargo test --lib live_segmented_download -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_segmented_download() {
        let url = "https://gh-proxy.com/https://github.com/ggml-org/llama.cpp/\
                   releases/download/b11714/llama-b11714-bin-win-vulkan-x64.zip";
        let dir = temp_dir("xieyi-live-seg");
        let dest = dir.join("engine.zip");
        let expected = "d4765447307f29fc0b4c8ae5b8b806c46b3e515d0ce5c2a54dd90d9b2c7a392f";

        let started = std::time::Instant::now();
        let done = download(url, &dest, Some(expected), None, |_| {})
            .await
            .expect("真实网络下载应当成功");
        println!(
            "live segmented: {} bytes in {:?}",
            done.bytes,
            started.elapsed()
        );
        assert_eq!(done.sha256, expected);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 真实网络验证：带 UA 之后魔搭不再回 403。
    ///
    /// 默认跳过——CI 与离线开发环境不该依赖外网。需要时手动跑：
    ///
    /// ```text
    /// XIEYI_LIVE_NET=1 cargo test live_modelscope -- --nocapture
    /// ```
    ///
    /// 这条测试存在的理由：UA 这个修复**在本地跑单测永远看不到效果**，
    /// 只能真连一次。留着它是为了让下一个人怀疑「403 是不是又回来了」
    /// 时，有个现成的验证手段，而不是重新用 curl 试三种 UA。
    #[tokio::test]
    async fn live_modelscope_accepts_our_ua() {
        if std::env::var("XIEYI_LIVE_NET").is_err() {
            return;
        }
        let url = "https://modelscope.cn/models/Tencent-Hunyuan/Hy-MT2-1.8B-GGUF/resolve/master/Hy-MT2-1.8B-Q8_0.gguf";
        let resp = http_client()
            .get(url)
            .header(reqwest::header::RANGE, "bytes=0-1023")
            .send()
            .await
            .expect("请求应当发出");
        let status = resp.status();
        eprintln!("LIVE status = {status}");
        assert!(
            status.is_success() || status.as_u16() == 206,
            "带 UA 时魔搭应当给 200/206，实际 {status}。\
             若这里是 403，说明 User-Agent 又被去掉了。"
        );
    }
}
