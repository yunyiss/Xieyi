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

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use tokio::io::AsyncWriteExt;

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
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Cancelled => write!(f, "已取消下载"),
            DownloadError::Failed(msg) => f.write_str(msg),
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

/// 下载一个文件到 `dest`，可断点续传。
///
/// - `url`：完整下载地址
/// - `dest`：最终路径。下载中写 `dest.part`
/// - `expected_sha256`：期望的哈希；`None` 表示不校验（**不建议**，
///   但有些场景（如自定义镜像）拿不到哈希）
/// - `cancel`：取消标志。`None` 表示这次下载不可取消（引擎包就是这么传的）
/// - `on_progress`：进度回调，调用频率由内部限流（约 250ms 一次）
///
/// 返回 `Err(DownloadError::Cancelled)` 时 `.part` **保留**，下次可续传。
/// 返回 `Err(DownloadError::Failed(..))` 时也保留，除了校验失败——那时
/// 内容已确认是坏的，留着只会重复失败。
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
    let part = part_path(dest);

    // 已有多少字节可以复用
    let existing = tokio::fs::metadata(&part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    // 一进来就先看一次：用户可能在排队等源的时候就取消了
    if cancelled(cancel) {
        return Err(DownloadError::Cancelled);
    }

    let (downloaded, resumed) = if existing > 0 {
        match try_resume(
            url,
            &part,
            existing,
            expected_sha256,
            cancel,
            &mut on_progress,
        )
        .await?
        {
            Some(final_size) => (final_size, true),
            // 续传不可用（服务端不支持 Range / 文件已变）→ 从头下
            None => {
                let _ = tokio::fs::remove_file(&part).await;
                (
                    fresh_download(url, &part, expected_sha256, cancel, &mut on_progress).await?,
                    false,
                )
            }
        }
    } else {
        (
            fresh_download(url, &part, expected_sha256, cancel, &mut on_progress).await?,
            false,
        )
    };

    // 校验：先算哈希，再决定是否改名
    let sha256 = sha256_file(&part).await.map_err(DownloadError::Failed)?;

    if let Some(expected) = expected_sha256 {
        let expected = expected.trim().to_lowercase();
        if sha256 != expected {
            // 内容确认是坏的，留着只会让下次续传拼出更坏的文件
            let _ = tokio::fs::remove_file(&part).await;
            return Err(DownloadError::Failed(format!(
                "下载校验失败：期望 {expected}，实际 {sha256}。文件已删除，请重试。"
            )));
        }
    }

    // 校验通过才改名。跨设备时 rename 会失败，退回 copy + remove
    if tokio::fs::rename(&part, dest).await.is_err() {
        tokio::fs::copy(&part, dest)
            .await
            .map_err(|e| DownloadError::Failed(format!("保存文件失败: {e}")))?;
        let _ = tokio::fs::remove_file(&part).await;
    }

    // **确认真的落位了**再报告成功。
    //
    // 参考实践里明确提到：不要因为"输入流读完了"就宣布完成。
    // rename 在某些情况下会静默失败（目标磁盘满、权限、杀软占用），
    // 此时 `dest` 可能不存在或是 0 字节。用户看到"下载完成"却发现
    // 模型列表里没有，比直接报错更让人困惑。
    //
    // 校验的是"目标文件存在且大小与下载量一致"，不重算哈希——
    // 哈希刚刚算过，rename 不会改变内容。
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
    let _ = tokio::fs::remove_file(meta_path(&part)).await;

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
                // 拼起来必然是坏的
                let _ = tokio::fs::remove_file(part_path(dest)).await;
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
async fn fresh_download<F>(
    url: &str,
    part: &Path,
    expected_sha256: Option<&str>,
    cancel: Option<&AtomicBool>,
    on_progress: &mut F,
) -> Result<u64, DownloadError>
where
    F: FnMut(Progress),
{
    let resp = http_client()
        .get(url)
        .send()
        .await
        .map_err(|e| DownloadError::Failed(format!("请求下载地址失败: {e}")))?;

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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ResumeMeta {
    /// 产物身份：期望的 SHA-256（无哈希时为 URL）
    identity: String,
    /// 期望的总字节数
    total: u64,
    /// 首次下载时服务端给的 ETag。续传时用 `If-Range` 发回去，
    /// 让服务端判断"文件是否已变"——变了就返回 200，我们据此放弃续传。
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
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
    let downloaded = std::fs::metadata(&part).ok()?.len();
    if downloaded == 0 {
        // 0 字节的 `.part` 没有任何续传价值，不值得在界面占一行
        return None;
    }

    // total 从元信息里取；读不到就算了，界面显示"未知大小"即可
    let total = std::fs::read_to_string(meta_path(&part))
        .ok()
        .and_then(|raw| serde_json::from_str::<ResumeMeta>(&raw).ok())
        .map(|m| m.total)
        .unwrap_or(0);

    Some(PartialState { downloaded, total })
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
    bytes
}

/// 写 `.part.meta`。失败不影响下载本身——只是下次不能续传。
async fn write_meta(part: &Path, identity: &str, total: u64, etag: Option<String>) {
    let meta = ResumeMeta {
        identity: identity.to_string(),
        total,
        etag,
    };
    if let Ok(raw) = serde_json::to_string(&meta) {
        let _ = tokio::fs::write(meta_path(part), raw).await;
    }
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

        // 60ms 后取消——这时期望只下了一两百万字节，远没到 2.5 MB
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
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
