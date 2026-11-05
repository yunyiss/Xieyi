// `Command` 只在 Windows / Linux 的 `query_nvidia_smi` 里用到；
// macOS 上该函数被 `#[cfg]` 排除，导入会变成 unused warning。
// 把 import 挂到同样的 cfg 上，而不是用 `#[allow(unused)]` 糊住——
// 后者会让真正的忘记使用也一起被静音。
#[cfg(any(target_os = "windows", target_os = "linux"))]
use std::process::Command;

/// 查询当前空闲显存（MB）。拿不到时返回 `None`，上层会退化成「全 GPU 层」。
///
/// ## 平台差异
///
/// - **Windows / Linux**：`nvidia-smi` 的参数完全一致，只是 Windows 上要加
///   `CREATE_NO_WINDOW` 避免弹黑框。
/// - **macOS**：没有 NVIDIA 驱动这条路，统一内存由系统管理，
///   这里的 `None` 会让上层退化成「全 GPU 层」——对 Apple Silicon 恰好是对的。
///
/// **只支持 NVIDIA 是有意的取舍**（技术债 #3）：AMD / Intel 要读各家
/// 专有接口，维护成本高于收益。拿不到就全量上 GPU，性能不是最优但能跑。
pub fn free_vram_mb() -> Option<u64> {
    query_nvidia_smi()
}

/// 调 `nvidia-smi` 读空闲显存。没有该命令（非 NVIDIA 机器）就返回 `None`。
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn query_nvidia_smi() -> Option<u64> {
    let mut cmd = Command::new("nvidia-smi");
    cmd.args(["--query-gpu=memory.free", "--format=csv,noheader,nounits"]);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }

    // 多卡时取第一块：路由进程只用一块卡，加起来会高估可用显存
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .parse::<u64>()
        .ok()
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn query_nvidia_smi() -> Option<u64> {
    // macOS：统一内存架构，没有独立的显存可查。
    // 返回 None 让上层退化成「全 GPU 层」，这对 Apple Silicon 是合理的默认。
    None
}
