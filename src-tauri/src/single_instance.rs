//! 单实例守护 —— **必须在创建 WebView 之前**执行。
//!
//! ## 为什么不能只用 `tauri-plugin-single-instance`
//!
//! 那个插件的互斥量检查发生在 `setup` 回调里，而 WebView 是在
//! `Builder::build()` 内部创建的，**早于 setup**。于是两个实例同时启动时：
//!
//! ```text
//! 实例A: 创建 WebView ──┐
//! 实例B: 创建 WebView ──┤ 两者都拿到了 EBWebView 目录
//!                       ↓
//!              ERROR_BUSY (0x800700AA)
//!                       ↓
//!            两个都 panic，谁都没起来
//! ```
//!
//! 实测确认过：同时启动两个进程，**A 和 B 都崩**，最终进程数为 0。
//! 用户快速双击图标就会触发，表现为"程序打不开"。
//!
//! ## 做法
//!
//! 在 `run()` 最开始（任何 Tauri 代码之前）拿一个**命名互斥量**：
//!
//! - 拿到了 → 本进程是唯一的，继续启动
//! - 已存在 → 说明有实例在跑，把它的窗口拉到前面，然后**立刻退出**
//!
//! 互斥量句柄故意不关闭（`Drop` 里只做 focus 不管句柄），
//! 由进程退出时由系统回收——这是 Windows 命名互斥量的标准用法。



/// 互斥量名字。带 `Local\` 前缀表示只在当前登录会话内互斥：
/// 同一台机器上的不同用户各开一个是合理的，不该互相拦。
///
/// 只在 Windows 下使用：Unix 侧尚未实现单实例（见 `acquire` 的注释）。
#[cfg(windows)]
const MUTEX_NAME: &str = "Local\\com.xieyi.translate.single-instance";

/// 单实例守卫。活着就代表本进程持有锁。
///
/// 不要提前 `drop` 它——句柄一关互斥量就释放，第二个实例就能进来了。
pub struct SingleInstance {
    #[cfg(windows)]
    _handle: isize,
    /// Unix：持有 flock 的文件句柄。**不要提前 drop**——
    /// 进程退出或句柄关闭时锁才释放，这是 flock 的语义。
    #[cfg(unix)]
    _lock_file: std::fs::File,
}

/// 尝试成为唯一实例。
///
/// - `Ok(guard)`：本进程是唯一实例，可以继续启动
/// - `Err(())`：已有实例在跑（窗口已被拉到前面），调用方应立即退出
pub fn acquire() -> Result<SingleInstance, ()> {
    #[cfg(windows)]
    {
        match windows_impl::try_lock(MUTEX_NAME) {
            Some(handle) => Ok(SingleInstance { _handle: handle }),
            None => {
                // 已有实例：把它的窗口拉到前台再退出。
                // 用户点了图标，本意是"我要用这个工具"，不是"再开一个"。
                windows_impl::focus_existing_window();
                Err(())
            }
        }
    }
    #[cfg(unix)]
    {
        match unix_impl::try_lock() {
            Some(file) => Ok(SingleInstance { _lock_file: file }),
            None => {
                // 已有实例：把它的窗口拉到前面再退出
                unix_impl::focus_existing_window();
                Err(())
            }
        }
    }
}

/// Unix 侧实现：`flock` 排他锁 + 窗口聚焦。
///
/// ## 为什么用 flock 而不是 PID 文件
///
/// PID 文件有两个坑：进程崩溃时不会自动清理（留下假锁），
/// PID 还会被系统复用（误判成"已有实例"）。
///
/// `flock` 是**内核级**的：持有者退出（哪怕被 `kill -9`）时锁自动释放，
/// 不存在清理问题。这是 Unix 上做单实例的标准做法。
#[cfg(unix)]
mod unix_impl {
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::os::unix::io::AsRawFd;

    /// 与 Windows 的 `Local\` 语义对齐：锁放在用户自己的运行时目录下，
    /// 不同用户各开一个是合理的（`/run/user/<uid>`），不该互相拦。
    pub fn lock_path() -> std::path::PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        dir.join("com.xieyi.translate.lock")
    }

    #[link(name = "c")]
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }

    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;

    /// 拿排他锁。已被占用返回 `None`。
    pub fn try_lock() -> Option<File> {
        let path = lock_path();

        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .ok()?;

        // LOCK_EX | LOCK_NB：排他 + 不阻塞。返回 -1 说明已有人持有。
        let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
        if rc != 0 {
            return None;
        }

        // 写入自己的 PID，方便排查（不影响锁本身）
        let _ = file.set_len(0);
        let _ = write!(file, "{}", std::process::id());
        let _ = file.flush();

        Some(file)
    }

    /// 聚焦已在运行的窗口。
    ///
    /// **尚未实现**：Linux 上没有跨桌面环境通用的"按 PID 找窗口并聚焦"接口
    /// （X11 可以走 `_NET_WM_PID` + `XRaiseWindow`，Wayland 出于安全考虑
    /// 基本禁止）。这里保持空实现，行为等同于「第二个实例静默退出」——
    /// 对用户来说至少不会开出两个抢显存的实例。
    ///
    /// 后续要补的话：X11 用 `xdotool search --pid <pid> windowactivate`，
    /// 需要检测 `XDG_SESSION_TYPE` 是否为 `x11`。
    pub fn focus_existing_window() {
        // 有意留空，原因见上
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const SW_RESTORE: i32 = 9;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(attrs: *const u8, initial_owner: i32, name: *const u16) -> isize;
        fn GetLastError() -> u32;
        fn GetCurrentProcessId() -> u32;
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        fn IsWindowVisible(hwnd: isize) -> i32;

        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> isize;
        fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(cb: extern "system" fn(isize, isize) -> i32, lparam: isize) -> i32;
        fn GetWindowTextLengthW(hwnd: isize) -> i32;
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
        fn IsIconic(hwnd: isize) -> i32;
    }

    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const INVALID_HANDLE_VALUE: isize = -1;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 拿命名互斥量。已有实例时返回 `None`。
    pub fn try_lock(name: &str) -> Option<isize> {
        let name = wide(name);
        unsafe {
            let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            if handle == 0 {
                // 创建失败（极罕见）：保守放行，不要因为保护机制本身坏了就不让用
                return Some(0);
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                // 别的实例持有；关掉我们这份句柄，返回"没拿到"
                CloseHandle(handle);
                return None;
            }
            Some(handle)
        }
    }

    static TARGET_PID: AtomicU32 = AtomicU32::new(0);
    static FOUND_HWND: AtomicIsize = AtomicIsize::new(0);

    extern "system" fn enum_cb(hwnd: isize, _lparam: isize) -> i32 {
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            // 没有标题的多半是隐藏的辅助窗口
            if GetWindowTextLengthW(hwnd) == 0 {
                return 1;
            }
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == TARGET_PID.load(Ordering::Relaxed) {
                FOUND_HWND.store(hwnd, Ordering::Relaxed);
                return 0; // 找到就停
            }
            1
        }
    }

    /// 找到已在运行的本程序窗口并拉到前台。
    pub fn focus_existing_window() {
        let Some(pid) = find_other_own_process() else {
            return;
        };
        TARGET_PID.store(pid, Ordering::Relaxed);
        FOUND_HWND.store(0, Ordering::Relaxed);

        unsafe {
            EnumWindows(enum_cb, 0);
            let hwnd = FOUND_HWND.load(Ordering::Relaxed);
            if hwnd == 0 {
                return;
            }
            // 最小化时必须先还原：只调 SetForegroundWindow 对最小化窗口无效
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd);
        }
    }

    /// 找出另一个同名 exe 的进程 PID（排除自己）。
    fn find_other_own_process() -> Option<u32> {
        let own = std::env::current_exe().ok()?;
        let own_stem = own
            .file_name()?
            .to_string_lossy()
            .trim_end_matches(".exe")
            .to_string();
        let self_pid = unsafe { GetCurrentProcessId() };

        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE || snap == 0 {
                return None;
            }

            let mut entry = std::mem::MaybeUninit::<ProcessEntry32W>::zeroed();
            (*entry.as_mut_ptr()).dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;

            let mut found = None;
            if Process32FirstW(snap, entry.as_mut_ptr()) != 0 {
                loop {
                    let e = entry.assume_init_ref();
                    let pid = e.th32_process_id;
                    if pid != self_pid {
                        let end = e
                            .sz_exe_file
                            .iter()
                            .position(|&c| c == 0)
                            .unwrap_or(e.sz_exe_file.len());
                        let name = String::from_utf16_lossy(&e.sz_exe_file[..end]);
                        if name
                            .trim_end_matches(".exe")
                            .eq_ignore_ascii_case(&own_stem)
                        {
                            found = Some(pid);
                            break;
                        }
                    }
                    entry = std::mem::MaybeUninit::zeroed();
                    (*entry.as_mut_ptr()).dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
                    if Process32NextW(snap, entry.as_mut_ptr()) == 0 {
                        break;
                    }
                }
            }

            CloseHandle(snap);
            found
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 用命名互斥量，必须带 `Local\` 前缀：
    /// 同一台机器上不同用户各开一个是合理的，不该互相拦。
    #[cfg(windows)]
    #[test]
    fn mutex_name_is_session_scoped() {
        assert!(
            MUTEX_NAME.starts_with("Local\\"),
            "互斥量应当限定在当前会话内，否则多用户会互相拦"
        );
        assert!(
            MUTEX_NAME.contains("com.xieyi.translate"),
            "互斥量名字要能唯一标识本应用"
        );
    }

    /// Unix 用 flock，锁文件必须落在**当前用户**的运行时目录里，
    /// 语义与 Windows 的 `Local\` 对齐。
    #[cfg(unix)]
    #[test]
    fn unix_lock_path_is_per_user() {
        let path = unix_impl::lock_path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            name.contains("com.xieyi.translate"),
            "锁文件名要能唯一标识本应用，实际是 {name}"
        );

        // XDG_RUNTIME_DIR 是 per-user 的（/run/user/<uid>）；
        // 没有的话退到 temp_dir，也仍然是当前用户可写
        if let Some(rt) = std::env::var_os("XDG_RUNTIME_DIR") {
            assert!(
                path.starts_with(std::path::PathBuf::from(rt)),
                "有 XDG_RUNTIME_DIR 时锁应放在里面，实际是 {path:?}"
            );
        }
    }

    #[test]
    fn first_acquire_succeeds() {
        // 测试进程内第一次拿应当是成功的
        let guard = acquire();
        assert!(guard.is_ok(), "首次获取单实例锁应当成功");
        drop(guard);
    }
}
