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

    // glibc 里 `flock` 属于 libc，要显式链接；macOS 的 libc 符号
    // 统一在 libSystem（std 已链接），再写 `#[link(name = "c")]`
    // 会找不到目标库。
    #[cfg_attr(target_os = "linux", link(name = "c"))]
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
    /// 聚焦已在运行的窗口。
    ///
    /// ## 平台差异
    ///
    /// **X11**：可以通过 `_NET_WM_PID` 属性按 PID 找到窗口，再
    /// `XRaiseWindow` + `XSetInputFocus` 前置。本实现走这条路。
    ///
    /// **Wayland**：协议层面禁止应用获取其他窗口的引用，也没有
    /// "raise 任意窗口"的接口。这不是能力缺失，是安全模型的设计选择。
    /// 因此 Wayland 下本函数静默返回，行为等同于「第二个实例直接退出」。
    ///
    /// ## 为什么用 dlopen 而不是链接 libX11
    ///
    /// 链接 `libX11` 会让二进制在**没有 X11 的机器上无法启动**
    /// （包括纯 Wayland 系统）。用 `dlopen` 动态加载：加载失败就跳过
    /// 聚焦，不影响启动。这与「聚焦是锦上添花」的定位一致。
    pub fn focus_existing_window() {
        // Wayland 下不做尝试：既没有可用接口，也没有绕过的手段
        if std::env::var("XDG_SESSION_TYPE")
            .map(|t| t.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false)
            || std::env::var_os("WAYLAND_DISPLAY").is_some()
        {
            crate::logging::info("single", "Wayland 会话，跳过窗口聚焦");
            return;
        }

        // 自己的 PID 用来排除；要找的是**另一个**同名进程
        let Some(pid) = find_other_own_process() else {
            return;
        };

        crate::logging::info("single", format!("尝试聚焦已是实例的窗口 pid={pid}"));

        if let Err(e) = raise_window_of_pid(pid) {
            crate::logging::warn("single", format!("聚焦窗口失败：{e}"));
        }
    }

    /// 聚焦指定 PID 的窗口。
    ///
    /// - **Linux**：X11 的 `_NET_ACTIVE_WINDOW`
    /// - **macOS**：Cocoa 的 `activateIgnoringOtherApps`
    /// - **其它 Unix**：不支持，静默跳过
    ///
    /// 聚焦是锦上添花——失败只记日志，绝不影响「第二个实例正常退出」
    /// 这条主路径。
    #[cfg(target_os = "linux")]
    fn raise_window_of_pid(pid: u32) -> Result<(), String> {
        x11::raise_window_of_pid(pid)
    }

    #[cfg(target_os = "macos")]
    fn raise_window_of_pid(pid: u32) -> Result<(), String> {
        macos::activate_app(pid)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn raise_window_of_pid(_pid: u32) -> Result<(), String> {
        Ok(())
    }

    /// 找出另一个同名可执行文件的进程 PID（排除自己）。
    ///
    /// **Linux**：读 `/proc/<pid>/exe` 的软链接目标做比较——这是
    /// Linux 上判定「同一个程序」最可靠的方式，比命令行字符串匹配更准。
    ///
    /// **macOS**：没有 `/proc`。改用 `ps -Ao pid=,comm=` 取每个进程的
    /// 可执行文件路径。`comm` 在 macOS 上给出完整路径，正好可以比较。
    #[cfg(target_os = "linux")]
    fn find_other_own_process() -> Option<u32> {
        let own = std::env::current_exe().ok()?;
        let own = std::fs::canonicalize(&own).ok()?;
        let self_pid = std::process::id();

        for entry in std::fs::read_dir("/proc").ok()? {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            if pid == self_pid {
                continue;
            }
            let exe_link = entry.path().join("exe");
            let Ok(target) = std::fs::canonicalize(&exe_link) else {
                // 无权限读别的用户的进程，正常，跳过
                continue;
            };
            if target == own {
                return Some(pid);
            }
        }
        None
    }

    #[cfg(target_os = "macos")]
    fn find_other_own_process() -> Option<u32> {
        let own = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
        let self_pid = std::process::id();

        let output = std::process::Command::new("ps")
            .args(["-Ao", "pid=,comm="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }

        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let line = line.trim_start();
            let Some((pid, comm)) = line.split_once(char::is_whitespace) else {
                continue;
            };
            let Ok(pid) = pid.parse::<u32>() else {
                continue;
            };
            if pid == self_pid {
                continue;
            }
            // comm 是完整路径；解析成规范形式再比，避免 /private/var
            // 与 /var 这类符号链接差异导致漏判
            if let Ok(target) = std::fs::canonicalize(comm.trim()) {
                if target == own {
                    return Some(pid);
                }
            }
        }
        None
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn find_other_own_process() -> Option<u32> {
        None
    }

    /// macOS 窗口聚焦。
    ///
    /// ## 为什么用 `NSRunningApplication` 而不是 AXUIElement
    ///
    /// macOS 没有「把某个 PID 的窗口拉到前面」这种直接 API。
    /// 官方推荐的无障碍接口（`AXUIElementPerformAction(kAXRaiseAction)`）
    /// **需要用户手动在「系统设置 → 隐私与安全性 → 辅助功能」里授权**，
    /// 一个翻译工具不该为「拉个窗口」要这种权限。
    ///
    /// 退一步用 `NSRunningApplication.activate`：它激活的是**整个应用**，
    /// 不指定具体窗口。我们的应用只有一个窗口，所以效果等价。
    /// 这个 API 不需要任何授权。
    ///
    /// ## 为什么用 objc_msgSend 手写
    ///
    /// 引 `objc2` / `cocoa` 会带进大量依赖。这里只要两次调用
    /// （`runningApplicationWithProcessIdentifier:` + `activateWithOptions:`），
    /// 手写 FFI 更划算——和本文件其它地方同一个思路。
    #[cfg(target_os = "macos")]
    mod macos {
        use std::ffi::c_void;

        /// `NSApplicationActivationOptions`：同时激活所有窗口
        const ACTIVATE_ALL_WINDOWS: usize = 1 << 0;

        #[link(name = "AppKit", kind = "framework")]
        unsafe extern "C" {}

        #[link(name = "objc")]
        unsafe extern "C" {
            fn objc_getClass(name: *const u8) -> *mut c_void;
            fn sel_registerName(name: *const u8) -> *mut c_void;
            fn objc_msgSend();
        }

        pub fn activate_app(pid: u32) -> Result<(), String> {
            /// 把 `objc_msgSend` 转成具体签名再调。
            ///
            /// 直接调 `objc_msgSend` 在 ARM64 上会崩：它是个可变参数
            /// 的跳板，必须 cast 成目标方法的真实签名。这里用到两种：
            /// - 类方法：`id (Class, SEL, int) -> id`
            /// - 实例方法：`BOOL (id, SEL, NSUInteger) -> BOOL`
            type MsgSendClassId = unsafe extern "C" fn(*mut c_void, *mut c_void, i32) -> *mut c_void;
            type MsgSendActivate =
                unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> i8;

            // SAFETY: 全部是 Foundation 的公开类与方法，参数类型与
            // Objective-C 声明一致。对象生命周期由 autorelease pool 之外的
            // 全局缓存管理（NSRunningApplication 是单例表），我们只读不持有。
            unsafe {
                let cls = objc_getClass(b"NSRunningApplication ".as_ptr());
                if cls.is_null() {
                    return Err("找不到 NSRunningApplication 类".into());
                }

                let sel_app = sel_registerName(
                    b"runningApplicationWithProcessIdentifier: ".as_ptr(),
                );
                let msg_class: MsgSendClassId = std::mem::transmute(objc_msgSend as *const ());
                let app = msg_class(cls, sel_app, pid as i32);
                if app.is_null() {
                    return Err(format!("PID {pid} 没有对应的 NSRunningApplication"));
                }

                let sel_activate = sel_registerName(b"activateWithOptions: ".as_ptr());
                let msg_activate: MsgSendActivate =
                    std::mem::transmute(objc_msgSend as *const ());
                let ok = msg_activate(app, sel_activate, ACTIVATE_ALL_WINDOWS);
                if ok == 0 {
                    return Err("activateWithOptions: 返回 NO".into());
                }
            }
            Ok(())
        }
    }

    /// X11 操作。全部通过 dlopen 动态解析符号。
    ///
    /// 只在 Linux 下编译：macOS 既没有 X11 运行时，也没有 `libdl`
    /// （`dlopen` 在 libSystem 里），编进去只会得到一个必然失败的
    /// 死模块。macOS 的聚焦走 [`macos::activate_app`]。
    #[cfg(target_os = "linux")]
    mod x11 {
        use std::ffi::{c_char, c_int, c_ulong, c_void, CString};

        /// `XWindowAttributes` 只需要 layout 正确，我们只用它的 map_state。
        /// 完整结构体字段很多，这里按 Xlib 的顺序声明到需要的字段为止。
        #[repr(C)]
        struct XWindowAttributes {
            x: c_int,
            y: c_int,
            width: c_int,
            height: c_int,
            border_width: c_int,
            depth: c_int,
            visual: *mut c_void,
            root: c_ulong,
            class: c_int,
            bit_gravity: c_int,
            win_gravity: c_int,
            backing_store: c_int,
            backing_planes: c_ulong,
            backing_pixel: c_ulong,
            save_under: c_int,
            colormap: c_ulong,
            map_installed: c_int,
            map_state: c_int,
            all_event_masks: CLong,
            your_event_mask: CLong,
            do_not_propagate_mask: CLong,
            override_redirect: c_int,
            screen: *mut c_void,
        }

        type CLong = i64;

        type QueryTreeFn = unsafe extern "C" fn(
            *mut c_void,
            c_ulong,
            *mut c_ulong,
            *mut c_ulong,
            *mut *mut c_ulong,
            *mut u32,
        ) -> c_int;
        type GetPropertyFn = unsafe extern "C" fn(
            *mut c_void,
            c_ulong,
            c_ulong,
            i64,
            i64,
            c_int,
            c_ulong,
            *mut c_ulong,
            *mut c_int,
            *mut c_ulong,
            *mut c_ulong,
            *mut *mut u8,
        ) -> c_int;
        type GetAttrsFn =
            unsafe extern "C" fn(*mut c_void, c_ulong, *mut XWindowAttributes) -> c_int;
        type FreeFn = unsafe extern "C" fn(*mut c_void) -> c_int;

        // X11 的 map_state 取值
        const IS_VIEWABLE: c_int = 2;

        // dlopen / dlsym。`libdl` 是 glibc 的东西；macOS 没有它，
        // 符号同样在 libSystem 里。这个 mod 本身只在 Linux 下编译
        // （见上面的 cfg），属性也一并收窄更保险。
        #[cfg_attr(target_os = "linux", link(name = "dl"))]
        unsafe extern "C" {
            fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
            fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
            fn dlerror() -> *mut c_char;
        }

        type SendEventFn = unsafe extern "C" fn(
            *mut c_void,
            c_ulong,
            c_int,
            CLong,
            *mut c_void,
        ) -> c_int;
        type InternAtomFn = unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_ulong;
        type FlushFn = unsafe extern "C" fn(*mut c_void) -> c_int;

        const RTLD_LAZY: c_int = 1;
        const RTLD_NOLOAD: c_int = 4;

        struct Lib {
            handle: *mut c_void,
        }

        impl Drop for Lib {
            fn drop(&mut self) {
                // 不用 dlclose：Xlib 有自己的全局状态，关掉可能影响正在
                // 运行的 WebView（GTK 也用 X11）。进程退出时由系统回收。
                let _ = self.handle;
            }
        }

        impl Lib {
            fn open() -> Result<Self, String> {
                // 先试 RTLD_NOLOAD：如果 GTK/WebKit 已经把 X11 加载进进程，
                // 直接复用，避免重复初始化
                for name in ["libX11.so.6 ", "libX11.so "] {
                    let cname = name.as_ptr() as *const c_char;
                    let h = unsafe { dlopen(cname, RTLD_LAZY | RTLD_NOLOAD) };
                    if !h.is_null() {
                        return Ok(Self { handle: h });
                    }
                    let h = unsafe { dlopen(cname, RTLD_LAZY) };
                    if !h.is_null() {
                        return Ok(Self { handle: h });
                    }
                }
                let msg = unsafe {
                    let e = dlerror();
                    if e.is_null() {
                        "libX11 加载失败".to_string()
                    } else {
                        std::ffi::CStr::from_ptr(e).to_string_lossy().to_string()
                    }
                };
                Err(msg)
            }

            fn sym(&self, name: &str) -> Result<*mut c_void, String> {
                let cname = CString::new(name).map_err(|e| e.to_string())?;
                // SAFETY: handle 来自 dlopen，name 是有效的 C 字符串
                let p = unsafe { dlsym(self.handle, cname.as_ptr()) };
                if p.is_null() {
                    return Err(format!("找不到符号 {name}"));
                }
                Ok(p)
            }
        }

        /// 通过 EWMH 的 `_NET_ACTIVE_WINDOW` 客户端消息请求 WM 激活窗口。
        ///
        /// 这是**唯一跨 WM 通用**的"把窗口带到前面"方式。直接 XRaiseWindow
        /// 或 XMapRaised 只能改变堆叠顺序，无法让 WM 取消最小化标记。
        ///
        /// 消息格式（EWMH 规范）：
        /// - data[0] = 1（来源：应用主动请求）
        /// - data[1] = CurrentTime
        /// - data[2] = 0（无需额外处理）
        #[allow(clippy::too_many_arguments)]
        unsafe fn send_activate_request(
            dpy: *mut c_void,
            root: c_ulong,
            window: c_ulong,
            x_send_event: SendEventFn,
            x_intern_atom: InternAtomFn,
            x_flush: FlushFn,
        ) {
            #[repr(C)]
            #[derive(Clone, Copy)]
            struct XClientMessageEvent {
                type_: c_int,
                serial: c_ulong,
                send_event: c_int,
                display: *mut c_void,
                window: c_ulong,
                message_type: c_ulong,
                format: c_int,
                data: [i64; 5],
            }

            let atom = unsafe { (x_intern_atom)(dpy, c"_NET_ACTIVE_WINDOW".as_ptr(), 0) };
            if atom == 0 {
                return;
            }

            let mut ev = XClientMessageEvent {
                type_: 33, // ClientMessage
                serial: 0,
                send_event: 1,
                display: dpy,
                window,
                message_type: atom,
                format: 32,
                // data[0]=2 表示"来自分页器"，比 1（应用主动请求）
                // 更强制；部分 WM（xfwm4）对 1 会做焦点窃取检查后忽略
                data: [2, 0, 0, 0, 0],
            };

            // SubstructureRedirectMask | SubstructureNotifyMask = 0x00140000
            unsafe {
                (x_send_event)(dpy, root, 0, 0x0014_0000, &mut ev as *mut _ as *mut c_void);
                (x_flush)(dpy);
            }
        }

        /// 递归遍历窗口树，找出属于 `pid` 且面积最大的窗口。
        ///
        /// **必须递归**：`XQueryTree` 只返回直接子窗口。Tauri 的主窗口
        /// 实测挂在第三层，只遍历顶层时完全找不到，只能找到 10x10 的
        /// 辅助窗口。
        ///
        /// 选面积最大的而非第一个：一个进程通常有多个窗口，
        /// 主窗口面积远大于辅助窗口。
        #[allow(clippy::too_many_arguments)]
        unsafe fn collect_best_window(
            dpy: *mut c_void,
            parent: c_ulong,
            pid: u32,
            net_wm_pid: c_ulong,
            xa_cardinal: c_ulong,
            x_query_tree: &QueryTreeFn,
            x_get_property: &GetPropertyFn,
            x_get_attrs: &GetAttrsFn,
            x_free: &FreeFn,
            best: &mut c_ulong,
            best_area: &mut i64,
            depth: u32,
        ) {
            // 防御异常深的树（正常桌面不会超过 10 层）
            if depth > 16 {
                return;
            }

            let mut root_ret: c_ulong = 0;
            let mut parent_ret: c_ulong = 0;
            let mut children: *mut c_ulong = std::ptr::null_mut();
            let mut nchildren: u32 = 0;

            if unsafe { (x_query_tree)(
                dpy,
                parent,
                &mut root_ret,
                &mut parent_ret,
                &mut children,
                &mut nchildren,
            ) } == 0
            {
                return;
            }

            for i in 0..nchildren as isize {
                let win = unsafe { *children.offset(i) };

                // 先看这个窗口是不是目标进程的
                let mut actual_type: c_ulong = 0;
                let mut actual_format: c_int = 0;
                let mut nitems: c_ulong = 0;
                let mut bytes_after: c_ulong = 0;
                let mut prop: *mut u8 = std::ptr::null_mut();

                let status = unsafe { (x_get_property)(
                    dpy,
                    win,
                    net_wm_pid,
                    0,
                    1,
                    0,
                    xa_cardinal,
                    &mut actual_type,
                    &mut actual_format,
                    &mut nitems,
                    &mut bytes_after,
                    &mut prop,
                ) };

                let mut matched = false;
                if status == 0 && nitems > 0 && !prop.is_null() {
                    let wpid = unsafe { *(prop as *const c_ulong) };
                    matched = wpid as u32 == pid;
                    unsafe { (x_free)(prop as *mut c_void) };
                }

                if matched {
                    let mut attrs: XWindowAttributes = unsafe { std::mem::zeroed() };
                    if unsafe { (x_get_attrs)(dpy, win, &mut attrs) } != 0
                        && attrs.width > 0
                        && attrs.height > 0
                    {
                        let area = attrs.width as i64 * attrs.height as i64;
                        let _ = depth; // 深度仅用于防御，不需要输出
                        if area > *best_area {
                            *best_area = area;
                            *best = win;
                        }
                    }
                }

                // 继续往下找（无论本层是否匹配）
                unsafe {
                    collect_best_window(
                    dpy,
                    win,
                    pid,
                    net_wm_pid,
                    xa_cardinal,
                    x_query_tree,
                    x_get_property,
                    x_get_attrs,
                    x_free,
                    best,
                        best_area,
                        depth + 1,
                    );
                }
            }

            if !children.is_null() {
                unsafe { (x_free)(children as *mut c_void) };
            }
        }

        /// 按 PID 找到可见窗口并前置。
        pub fn raise_window_of_pid(pid: u32) -> Result<(), String> {
            let lib = Lib::open()?;

            unsafe {
                // 函数指针签名（Xlib）
                type XOpenDisplay = unsafe extern "C" fn(*const c_char) -> *mut c_void;
                type XInternAtom =
                    unsafe extern "C" fn(*mut c_void, *const c_char, c_int) -> c_ulong;
                type XQueryTree = unsafe extern "C" fn(
                    *mut c_void,
                    c_ulong,
                    *mut c_ulong,
                    *mut c_ulong,
                    *mut *mut c_ulong,
                    *mut u32,
                ) -> c_int;
                type XGetWindowProperty = unsafe extern "C" fn(
                    *mut c_void,
                    c_ulong,
                    c_ulong,
                    i64,
                    i64,
                    c_int,
                    c_ulong,
                    *mut c_ulong,
                    *mut c_int,
                    *mut c_ulong,
                    *mut c_ulong,
                    *mut *mut u8,
                ) -> c_int;
                type XGetWindowAttributes =
                    unsafe extern "C" fn(*mut c_void, c_ulong, *mut XWindowAttributes) -> c_int;
                        type XMapRaised = unsafe extern "C" fn(*mut c_void, c_ulong) -> c_int;
                type XSetInputFocus =
                    unsafe extern "C" fn(*mut c_void, c_ulong, c_int, c_ulong) -> c_int;
                        type XFlush = unsafe extern "C" fn(*mut c_void) -> c_int;
                type XSendEvent = unsafe extern "C" fn(
                    *mut c_void,
                    c_ulong,
                    c_int,
                    CLong,
                    *mut c_void,
                ) -> c_int;
                type XSync = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
                type XFree = unsafe extern "C" fn(*mut c_void) -> c_int;
                type XCloseDisplay = unsafe extern "C" fn(*mut c_void) -> c_int;

                let x_open_display: XOpenDisplay = std::mem::transmute(lib.sym("XOpenDisplay")?);
                let x_intern_atom: XInternAtom = std::mem::transmute(lib.sym("XInternAtom")?);
                let x_query_tree: XQueryTree = std::mem::transmute(lib.sym("XQueryTree")?);
                let x_get_property: XGetWindowProperty =
                    std::mem::transmute(lib.sym("XGetWindowProperty")?);
                let x_get_attrs: XGetWindowAttributes =
                    std::mem::transmute(lib.sym("XGetWindowAttributes")?);
                let x_focus: XSetInputFocus = std::mem::transmute(lib.sym("XSetInputFocus")?);
                let x_map_raised: XMapRaised = std::mem::transmute(lib.sym("XMapRaised")?);
                let x_flush: XFlush = std::mem::transmute(lib.sym("XFlush")?);
                let x_send_event: XSendEvent = std::mem::transmute(lib.sym("XSendEvent")?);
                let x_sync: XSync = std::mem::transmute(lib.sym("XSync")?);
                let x_free: XFree = std::mem::transmute(lib.sym("XFree")?);
                let x_close: XCloseDisplay = std::mem::transmute(lib.sym("XCloseDisplay")?);

                let dpy = x_open_display(std::ptr::null());
                if dpy.is_null() {
                    return Err("无法连接 X server（DISPLAY 未设置？）".into());
                }

                // 遍历整棵窗口树，找 _NET_WM_PID == 目标 pid 的窗口
                let net_wm_pid = x_intern_atom(dpy, c"_NET_WM_PID".as_ptr(), 0);
                let xa_cardinal = 6u64; // XA_CARDINAL

                // Screen 的 root 需要 XDefaultRootWindow，这里用 XQueryTree 传 0 不行，
                // 改用 XDefaultRootWindow 符号
                type XDefaultRootWindow = unsafe extern "C" fn(*mut c_void) -> c_ulong;
                let x_default_root: XDefaultRootWindow =
                    std::mem::transmute(lib.sym("XDefaultRootWindow")?);
                let root_win = x_default_root(dpy);

                // **必须递归遍历整棵窗口树**：XQueryTree 只返回直接子窗口，
                // 而 Tauri 的主窗口可能挂在更深的层级下。
                // 实测：只遍历顶层时只能找到 10x10 的辅助窗口，
                // 1080x720 的主窗口完全找不到。
                let mut best: c_ulong = 0;
                let mut best_area: i64 = -1;

                collect_best_window(
                    dpy,
                    root_win,
                    pid,
                    net_wm_pid,
                    xa_cardinal,
                    &x_query_tree,
                    &x_get_property,
                    &x_get_attrs,
                    &x_free,
                    &mut best,
                    &mut best_area,
                    0,
                );

                let found = best;
                if found == 0 {
                    x_close(dpy);
                    return Err(format!("pid={pid} 没有可用的窗口"));
                }

                // **最小化不是单纯的 unmapped**：窗口管理器会在
                // `_NET_WM_STATE` 里记一个 `_NET_WM_STATE_HIDDEN`。
                // 只调 XMapRaised 会让窗口重新 map，但 WM 仍认为它是
                // 最小化状态，于是立刻又收起来。
                //
                // 正确做法是走 EWMH 的标准流程：向 root 窗口发一条
                // `_NET_ACTIVE_WINDOW` 客户端消息，由 WM 负责取消最小化
                // 并聚焦。这是唯一跨 WM 通用的方式。
                send_activate_request(
                    dpy,
                    root_win,
                    found,
                    x_send_event,
                    x_intern_atom,
                    x_flush,
                );

                // 兜底：有些 WM 不响应激活请求，直接 map 一次
                x_map_raised(dpy, found);
                x_flush(dpy);

                // **等映射真正生效再设焦点**。
                // XSetInputFocus 对尚未 map 的窗口返回 BadMatch，
                // 而 map 是异步的——XFlush 只是把请求发出去。
                //
                // 轮询 map_state 直到 IsViewable：通常 1-2 次就够，
                // 上限 20 次（约 200ms）防止 X server 异常时卡死。
                let mut tries = 0;
                loop {
                    let mut attrs: XWindowAttributes = std::mem::zeroed();
                    let ok = x_get_attrs(dpy, found, &mut attrs) != 0;
                    if ok && attrs.map_state == IS_VIEWABLE {
                        break;
                    }
                    tries += 1;
                    if tries > 20 {
                        x_close(dpy);
                        return Err("窗口映射超时".into());
                    }
                    // XSync 让 X server 处理完已发出的请求
                    x_sync(dpy, 0);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }

                // RevertToParent = 2, CurrentTime = 0
                x_focus(dpy, found, 2, 0);
                x_flush(dpy);
                x_close(dpy);

                Ok(())
            }
        }
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
