//! Windows Job Object：让推理进程树随主进程一起消失。
//!
//! ## 解决什么问题
//!
//! 主进程正常退出时，`Service::kill()` 会 `taskkill /T` 清理整棵进程树。
//! 但这条路径只在**正常退出**时执行。以下情况它跑不到：
//!
//! - 主进程被 `TerminateProcess` 强杀（任务管理器「结束任务」）
//! - 主进程崩溃（panic + abort）
//! - 系统进入睡眠/关机时的强制终止
//!
//! 此时 `llama-server` 及其子进程成为孤儿。它们仍持有显存与端口，
//! 用户再次启动时会撞上 `ERROR_BUSY`，只能重启电脑解决。
//!
//! Job Object 把这个保证从「应用层尽力而为」下沉到「内核层强制执行」：
//! 进程一旦加入 Job，主进程句柄关闭时内核自动终止组内所有进程——
//! **无论主进程是正常退出还是被强杀**。
//!
//! ## 为什么不用 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
//!
//! 这里用的正是这个标志。它的语义是：**最后一个 Job 句柄关闭时**
//! 终止所有关联进程。主进程持有这个句柄，进程死亡时 Windows 会关闭
//! 它的所有句柄，于是 Job 被触发。这正是我们需要的。
//!
//! 注意是「最后一个句柄」——子进程继承句柄会让它存活得更久。
//! 所以创建 Job 时用 `CreateJobObjectW(null, null)` 拿匿名 Job，
//! 并**不把句柄设为可继承**（默认不可继承），避免被 llama-server
//! 的子进程继承。

/// 持有 Job Object 句柄的守卫。
///
/// 生命周期结束时关闭句柄，触发 `KILL_ON_JOB_CLOSE`。
/// **不要提前 drop**：那会立刻杀掉推理进程。
#[cfg(windows)]
pub struct JobHandle {
    handle: isize,
}

#[cfg(windows)]
impl JobHandle {
    /// 把已启动的进程加入 Job。
    ///
    /// 必须在 `spawn()` 之后**立刻**调用。虽然 llama-server 启动到
    /// 派生子进程之间有时间窗口，但窗口很小；真要消除它需要在
    /// 挂起状态下启动进程再恢复，代价远大于收益。
    ///
    /// 失败时只记日志、不阻断启动——Job 是「更好」的清理保证，
    /// 不是程序能跑起来的前提。拿不到 Job 时仍回退到 `taskkill /T`。
    pub fn assign(pid: u32) -> Option<Self> {
        match win::create_job_for(pid) {
            Some(handle) => {
                crate::logging::info("job", format!("推理进程 pid={pid} 已加入 Job Object"));
                Some(Self { handle })
            }
            None => {
                crate::logging::warn(
                    "job",
                    format!("无法为 pid={pid} 创建 Job Object，回退到 taskkill 清理"),
                );
                None
            }
        }
    }
}

#[cfg(windows)]
impl Drop for JobHandle {
    fn drop(&mut self) {
        win::close_handle(self.handle);
    }
}

/// 非 Windows 平台：没有 Job Object，返回空类型让调用处无需 cfg。
///
/// Unix 侧的等价保证来自进程组（`process_group(0)` + `killpg`）；
/// Linux 额外有 `PR_SET_PDEATHSIG`，**macOS 没有**（`prctl` 是 Linux
/// 专有接口）。两台 Unix 都还有「下次启动时按 api-key 扫掉残留」
/// 这层兜底，所以 macOS 上最坏情况是「强杀后重启前」会看到旧进程
/// 占着显存，不会一直泄漏。
#[cfg(not(windows))]
pub struct JobHandle;

#[cfg(not(windows))]
impl JobHandle {
    pub fn assign(_pid: u32) -> Option<Self> {
        // Unix 的进程组方案在 spawn 时建立、kill 时按组发送信号，
        // 语义等价。这里返回 None 表示「没有额外的 Job 需要持有」。
        None
    }
}

#[cfg(windows)]
mod win {
    use std::mem::MaybeUninit;

    const PROCESS_SET_QUOTA: u32 = 0x0100;
    const PROCESS_TERMINATE: u32 = 0x0001;

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const JOB_OBJECT_LIMIT_BREAKAWAY_OK: u32 = 0x0000_0800;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;

    #[repr(C)]
    #[derive(Default)]
    struct JobObjectBasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct JobObjectExtendedLimitInformation {
        basic_limit_information: JobObjectBasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attrs: *const u8, name: *const u16) -> isize;
        fn SetInformationJobObject(job: isize, info_class: i32, info: *const u8, len: u32) -> i32;
        fn AssignProcessToJobObject(job: isize, process: isize) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn CloseHandle(handle: isize) -> i32;
    }

    pub fn close_handle(handle: isize) {
        if handle != 0 && handle != -1 {
            // SAFETY: 句柄来自本模块的 CreateJobObjectW / OpenProcess，
            // 且只关闭一次（由调用方保证不重复 drop）
            unsafe { CloseHandle(handle) };
        }
    }

    /// 创建 Job、设置 `KILL_ON_JOB_CLOSE`、把 pid 加入。
    pub fn create_job_for(pid: u32) -> Option<isize> {
        // SAFETY: 全部为 Win32 Job Object API 的标准调用序列。
        // 任一环节失败都会关闭已创建的句柄，不留悬空资源。
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job == 0 {
                return None;
            }

            let mut info = MaybeUninit::<JobObjectExtendedLimitInformation>::zeroed().assume_init();
            info.basic_limit_information.limit_flags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;

            let ok = SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                &info as *const _ as *const u8,
                std::mem::size_of::<JobObjectExtendedLimitInformation>() as u32,
            );
            if ok == 0 {
                CloseHandle(job);
                return None;
            }

            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            if process == 0 {
                CloseHandle(job);
                return None;
            }

            let assigned = AssignProcessToJobObject(job, process);
            CloseHandle(process);

            if assigned == 0 {
                CloseHandle(job);
                return None;
            }

            Some(job)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 非 Windows 上 `assign` 返回 None 是设计行为（进程组已覆盖）。
    /// Windows 上传入不存在的 PID 应当安全失败而不是 panic。
    #[test]
    fn assigning_invalid_pid_does_not_panic() {
        let result = JobHandle::assign(u32::MAX);
        // 不存在的 PID 不该拿到 Job
        assert!(result.is_none(), "不存在的 PID 不应创建出 Job");
    }
}
