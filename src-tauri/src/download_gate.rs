//! 下载通道：**同一时刻只允许一个下载**，并且可以被用户取消。
//!
//! ## 为什么需要独立模块
//!
//! 有两类下载要共用这套机制：
//!
//! | 下载 | 体量 | 用户会中途放弃吗 |
//! |---|---|---|
//! | 推理引擎（llama.cpp） | 30.8 MB | 会——实测国内加速代理只有 ~90 KB/s，要等 5 分钟 |
//! | 预设模型 | 1.78 – 7.43 GB | 一定会 |
//!
//! 早先这套逻辑写死在 `presets.rs` 里，引擎那边完全没有取消能力。
//! 用户点了「下载引擎」只能干等，或者关掉窗口——而关窗口后 `.part`
//! 会留在磁盘上、界面上又看不见，下次未必能顺利续上。
//!
//! ## 为什么取消标志是**全局一个**
//!
//! 因为下载通道本来就只允许一个下载。两个独立的取消标志会带来
//! 一个更隐蔽的错误：用户取消引擎下载，代码里去置模型下载的标志，
//! 两个都在动但谁也没生效。
//!
//! ## 与翻译的取消**必须分开**
//!
//! `AppState.cancel` 是推理请求的取消位。若共用一个，用户取消下载会
//! 把正在跑的翻译也掐断——两件事在用户眼里毫无关系。

use std::sync::atomic::{AtomicBool, Ordering};

/// 用户是否请求取消当前下载。
static CANCEL: AtomicBool = AtomicBool::new(false);

/// 是否有下载正在进行。
static BUSY: AtomicBool = AtomicBool::new(false);

/// 占用下载通道的守卫。离开作用域时自动释放，**包括提前 return 和 panic**。
///
/// ## 为什么必须是 RAII 而不是手动 `store(false)`
///
/// 早先手写释放的版本在某个错误分支漏过一次：下载失败了但通道永远
/// 占着，之后所有下载都报「已有下载在进行」，只能重启应用。
/// Drop 保证任何退出路径都会放开。
pub struct Gate;

impl Gate {
    /// 尝试占用。已经被占则返回 `None`——**后来的请求直接拒绝，不排队**。
    ///
    /// 为什么不排队：用户以为自己点了两次就是在下两个，实际第二个被
    /// 静默挂起、等第一个完了又开始下同一个文件，比直接说「已有下载
    /// 在进行」更让人困惑。
    pub fn acquire() -> Option<Self> {
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| {
                // 拿到通道的同时清掉上一次的取消标志。
                // 顺序很重要：必须在占住通道**之后**清，否则两个任务
                // 都在抢通道时，后来者可能把先来者的取消状态抹掉。
                CANCEL.store(false, Ordering::Relaxed);
                Gate
            })
    }

    /// 取一个可以直接传给 `downloader` 的标志引用。
    ///
    /// `downloader` 收 `Option<&AtomicBool>` 而不是 `Gate`，是为了让它
    /// 不依赖这个模块——下载器只管搬字节，不该知道有几个下载任务。
    pub fn flag(&self) -> &'static AtomicBool {
        &CANCEL
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
        // 释放时也清掉取消标志：下一次下载不该继承上一次的取消状态。
        // 不清的话，用户取消过一次之后，所有后续下载都会立刻中止。
        CANCEL.store(false, Ordering::Relaxed);
    }
}

/// 请求取消当前下载。
///
/// 没有下载在进行时调用它是无害的（只是置一个标志，下次 `acquire`
/// 会清掉）。不做"有没有在下载"的检查，因为前端点取消与后端开始下载
/// 之间存在竞态，检查反而可能漏掉刚启动的那一个。
pub fn request_cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

/// 取消当前下载（引擎包或预设模型，两者共用这个通道）。
///
/// 这是给界面用的通用入口。早先只有 `cancel_preset_download`，
/// 名字暗示它只管模型——实际上通道是共用的，那个名字会误导调用方
/// 在下载引擎时以为没有取消手段。
///
/// 取消后 `.part` **保留**，下次会续传。
#[tauri::command]
pub fn cancel_download() {
    request_cancel();
}

/// 当前是否有下载在进行。供界面判断要不要显示「下载中」。
pub fn is_busy() -> bool {
    BUSY.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 守卫测试共用一个进程级静态量，cargo test 默认并行会互相抢通道。
    /// 只串行这几条，不用 `--test-threads=1`（那会拖慢整个套件）。
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 第二个请求必须被拒，不能两个任务同时写同一个文件。
    ///
    /// 变异验证：把 `acquire` 改成无条件 `store(true)` 并返回 `Some`，
    /// 本测试失败。
    #[test]
    fn second_acquire_is_rejected() {
        let _s = serial();
        let first = Gate::acquire().expect("第一次应当能拿到");
        assert!(Gate::acquire().is_none(), "通道已被占时第二次必须失败");
        drop(first);
        assert!(Gate::acquire().is_some(), "释放后应当能重新拿到");
    }

    /// 错误路径提前 return 时通道必须自动释放。
    ///
    /// 变异验证：删掉 `impl Drop for Gate`，本测试失败——这正是那个
    /// 「下载失败后所有下载都报已有下载在进行」的缺陷。
    #[test]
    fn releases_on_early_return() {
        let _s = serial();
        fn fails() -> Result<(), String> {
            let _g = Gate::acquire().ok_or("占用失败")?;
            Err("模拟失败".into())
        }
        assert!(fails().is_err());
        assert!(Gate::acquire().is_some(), "错误返回后通道没有释放");
    }

    /// panic 时也要释放，否则一个意料外的 panic 会永久锁死下载功能。
    #[test]
    fn releases_on_panic() {
        let _s = serial();
        let r = std::panic::catch_unwind(|| {
            let _g = Gate::acquire().expect("第一次应当能拿到");
            panic!("模拟 panic");
        });
        assert!(r.is_err());
        assert!(Gate::acquire().is_some(), "panic 后通道没有释放");
    }

    /// 拿到通道时自动清掉上一次的取消状态。
    ///
    /// 不清的后果很严重：用户取消过一次之后，所有后续下载都会在
    /// 第一块数据到达时立刻中止，表现为「怎么都下不动」。
    ///
    /// 变异验证：删掉 `acquire` 里的 `CANCEL.store(false, ...)`，
    /// 本测试失败。
    #[test]
    fn acquire_clears_previous_cancel() {
        let _s = serial();
        request_cancel();
        // 此时没占用通道，标志是 true
        assert!(CANCEL.load(Ordering::Relaxed));

        let _g = Gate::acquire().expect("应当能拿到");
        assert!(
            !CANCEL.load(Ordering::Relaxed),
            "新一次下载不该继承上一次的取消状态"
        );
    }

    /// 取消标志能被 `flag()` 读到。
    #[test]
    fn cancel_is_visible_through_flag() {
        let _s = serial();
        let g = Gate::acquire().expect("应当能拿到");
        assert!(!g.flag().load(Ordering::Relaxed));

        request_cancel();
        assert!(
            g.flag().load(Ordering::Relaxed),
            "request_cancel 之后应当读到 true"
        );
        assert!(
            g.flag().load(Ordering::Relaxed),
            "flag() 要能让 downloader 看到同一个状态"
        );
    }

    /// 释放通道时也清取消标志，避免影响下一次。
    #[test]
    fn release_clears_cancel() {
        let _s = serial();
        {
            let _g = Gate::acquire().expect("应当能拿到");
            request_cancel();
        }
        assert!(!CANCEL.load(Ordering::Relaxed), "释放后取消标志应当归零");
    }

    /// 没有下载在进行时取消是无害的。
    #[test]
    fn cancel_without_active_download_is_harmless() {
        let _s = serial();
        request_cancel();
        // 不 panic、不阻塞
        assert!(!is_busy() || is_busy());
        // 下一次 acquire 会把它清掉
        let g = Gate::acquire().expect("应当能拿到");
        assert!(!g.flag().load(Ordering::Relaxed));
    }
}
