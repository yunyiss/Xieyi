//! GPU 显存探测。
//!
//! ## 为什么不再只依赖 nvidia-smi
//!
//! 技术债 #3：AMD / Intel 用户拿不到显存信息，自动层数退化成「全部层都上 GPU」。
//! 性能不是最优，但更要紧的是**显存不够时会直接加载失败**，用户只看到
//! 一句「引擎启动失败」，完全不知道原因是显存装不下。
//!
//! ## 改用 llama.cpp 自己的 `--list-devices`
//!
//! 这个接口比调各家厂商工具好得多：
//!
//! - **后端无关**：CUDA / Vulkan / ROCm / Metal 都能报，不用为每种卡
//!   维护一套解析（`rocm-smi` 的输出格式还按版本变过）
//! - **单位统一**：直接给 MiB，不用处理 `rocm-smi` 的百分比/字节混用
//! - **和我们用的是同一个引擎**：它说能放下，就真的能放下
//!
//! 输出形如：
//!
//! ```text
//! Available devices:
//!   CUDA0: NVIDIA GeForce RTX 4060 Laptop GPU (8187 MiB, 7106 MiB free)
//!   Vulkan0: AMD Radeon RX 7800 XT (16384 MiB, 15200 MiB free)
//! ```
//!
//! ## 兜底顺序
//!
//! 1. `llama-server --list-devices`（首选，任何后端）
//! 2. `nvidia-smi`（老引擎 / 手动指定的非 llama.cpp 程序）
//! 3. `None` → 上层退化成全 GPU 层

use std::path::Path;
// `Command` 在三个平台都用得到：
//
// - `query_list_devices`（`--list-devices`）**无条件**使用它——
//   Metal / ROCm / Vulkan 都靠这条路径拿显存
// - `query_nvidia_smi` 只在 Windows / Linux 有
//
// 早期这里挂过 `#[cfg(any(windows, linux))]`，那时只有 `nvidia-smi`
// 会用 `Command`，挂 cfg 是对的。后来加了 `--list-devices` 却没同步
// 放宽 cfg，于是 **macOS 上编译直接失败**（`cannot find type Command`）。
// 因为本地只在 Windows 开发、CI 也没跑过 macOS，这个错误潜伏了很久。
use std::process::Command;

/// 查询当前空闲显存（MB）。拿不到时返回 `None`，上层会退化成「全 GPU 层」。
///
/// `server` 是当前在用的推理引擎路径。传 `None` 表示「还不知道引擎在哪」，
/// 此时只能用通用的 `nvidia-smi` 兜底。
pub fn free_vram_mb(server: Option<&Path>) -> Option<u64> {
    free_vram_mb_conservative(server)
}

/// 两路都能拿到时取**较小值**。
///
/// 实测发现两者会差几百 MB（RTX 4060 上：
/// `nvidia-smi` 报 6144，引擎报 7106，相差 ~960）。
/// 原因是两边对“已用”的口径不同：驱动把桌面等
/// 非计算进程也算进去，引擎只看自己能分配多少。
///
/// 我们**取小值**：估低只会少放几层（慢一点，但能跑），
/// 估高会直接加载失败（用户只看到一句「引擎启动失败」，
/// 完全猜不到原因是显存不够）。宁可慢，不可挂。
pub fn free_vram_mb_conservative(server: Option<&Path>) -> Option<u64> {
    let by_engine = server.and_then(query_list_devices);
    let by_smi = query_nvidia_smi();
    pick_smaller(by_engine, by_smi)
}

/// 两路取小。抽成独立函数是为了能单测：
/// 真实路径需要两个真实源，测试机不一定同时具备。
fn pick_smaller(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

/// 从 `llama-server --list-devices` 的输出里取第一块设备的空闲显存。
///
/// 只取第一块：路由进程只用一块卡，把多卡加起来会高估可用显存，
/// 反而导致层数算多了、加载时爆掉。
fn query_list_devices(server: &Path) -> Option<u64> {
    let mut cmd = Command::new(server);
    cmd.arg("--list-devices");

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let output = cmd.output().ok()?;
    // 有的版本即使列不出设备也返回 0，所以不能只看退出码，
    // 还要能从输出里真的解析出数字
    let text = String::from_utf8_lossy(&output.stdout);
    let text2 = String::from_utf8_lossy(&output.stderr);
    parse_free_mb(&text).or_else(|| parse_free_mb(&text2))
}

/// 从 `--list-devices` 的输出文本里解析第一块设备的空闲 MiB。
///
/// 单独拆出来是为了可测：真正的测试机不一定有异构 GPU，
/// 但解析逻辑必须对三种后端格式都正确。
fn parse_free_mb(text: &str) -> Option<u64> {
    for line in text.lines() {
        // 形如 `  CUDA0: NVIDIA ... (8187 MiB, 7106 MiB free)`
        // 也见过 macOS 的 `  Metal0: Apple M2 (16384 MiB, 12000 MiB free)`
        if !line.contains(" MiB free") {
            continue;
        }
        // 取最后一个 "N MiB free" 里的 N。
        // 从右往左找 " MiB free"，避免设备名里恰好含数字造成误取。
        let idx = line.rfind(" MiB free")?;
        let before = &line[..idx];
        // before 结尾是 "7106" 这样的数字，向前找到非数字为止
        let num: String = before
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if num.is_empty() {
            continue;
        }
        if let Ok(v) = num.parse::<u64>() {
            return Some(v);
        }
    }
    None
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
    // （Metal 的 --list-devices 会报一个值，走上面那条路径即可。）
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cuda_device_line() {
        let out = "Available devices:\n  CUDA0: NVIDIA GeForce RTX 4060 Laptop GPU (8187 MiB, 7106 MiB free)\n";
        assert_eq!(parse_free_mb(out), Some(7106));
    }

    #[test]
    fn parses_vulkan_amd_device_line() {
        // 技术债 #3 的目标场景：AMD 卡走 Vulkan 后端
        let out =
            "Available devices:\n  Vulkan0: AMD Radeon RX 7800 XT (16384 MiB, 15200 MiB free)\n";
        assert_eq!(parse_free_mb(out), Some(15200));
    }

    #[test]
    fn parses_intel_vulkan_device_line() {
        // Intel Arc：同样是 Vulkan 后端
        let out = "Available devices:\n  Vulkan0: Intel(R) Arc(TM) A770 Graphics (16384 MiB, 12000 MiB free)\n";
        assert_eq!(parse_free_mb(out), Some(12000));
    }

    #[test]
    fn parses_metal_device_line() {
        let out = "Available devices:\n  Metal0: Apple M2 Pro (16384 MiB, 14000 MiB free)\n";
        assert_eq!(parse_free_mb(out), Some(14000));
    }

    #[test]
    fn takes_first_device_only() {
        // 多卡必须只取第一块：加起来会高估，导致层数算多后加载爆掉
        let out = "Available devices:\n  CUDA0: A (8187 MiB, 7106 MiB free)\n  CUDA1: B (8187 MiB, 7000 MiB free)\n";
        assert_eq!(parse_free_mb(out), Some(7106));
    }

    #[test]
    fn ignores_lines_without_free_marker() {
        // CPU-only 构建会列出 `CPU: ...`，没有 MiB free，必须跳过
        let out = "Available devices:\n  CPU: Intel Core i9 (32 threads, 32768 MiB)\n";
        assert_eq!(parse_free_mb(out), None);
    }

    #[test]
    fn handles_no_devices() {
        assert_eq!(parse_free_mb("Available devices:\n"), None);
        assert_eq!(parse_free_mb(""), None);
    }

    #[test]
    fn tolerates_extra_spacing_and_crlf() {
        // Windows 输出带 \r\n
        let out = "Available devices:\r\n  CUDA0: X (8187 MiB,  7106 MiB free)\r\n";
        assert_eq!(parse_free_mb(out), Some(7106));
    }

    #[test]
    fn device_name_with_digits_does_not_confuse_parser() {
        // 型号里含数字（RTX 4060）不能把 4060 当成空闲显存
        let out = "  CUDA0: NVIDIA GeForce RTX 4060 Laptop GPU (8187 MiB, 7106 MiB free)";
        assert_eq!(parse_free_mb(out), Some(7106));
    }

    #[test]
    fn conservative_takes_smaller_of_two_sources() {
        // 实测：RTX 4060 上 nvidia-smi 报 6144、引擎报 7106。
        // 取小值 —— 估低只会慢，估高会加载失败。
        assert_eq!(pick_smaller(Some(7106), Some(6144)), Some(6144));
        assert_eq!(pick_smaller(Some(6144), Some(7106)), Some(6144));
    }

    #[test]
    fn conservative_handles_one_source_missing() {
        assert_eq!(pick_smaller(Some(8000), None), Some(8000));
        assert_eq!(pick_smaller(None, Some(6000)), Some(6000));
        assert_eq!(pick_smaller(None, None), None);
    }

    #[test]
    fn zero_free_is_reported_not_ignored() {
        // 显存被占满时是真实的 0，要如实返回——上层会据此选 CPU 兜底
        let out = "  CUDA0: X (8187 MiB, 0 MiB free)";
        assert_eq!(parse_free_mb(out), Some(0));
    }
}
