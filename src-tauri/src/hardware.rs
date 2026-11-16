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

/// 查询 GPU **总显存**（MB），不依赖已安装的引擎。
///
/// ## 与 `free_vram_mb` 的区别
///
/// `free_vram_mb` 首选 `llama-server --list-devices`，那是"当前能分到多少"，
/// 会随桌面/其他程序占用浮动。但它**需要引擎已经在磁盘上**——而引导层
/// 恰恰是"还没下引擎"的时刻，用来判断"这档模型我这台机器跑不跑得动"
/// 就赶不上了。
///
/// 这个函数只用系统自带信息，**任何时候都能调到**，给出的是硬件的
/// 总容量。用于预设卡片的「够不够」提示。
///
/// ## 各平台走的路
///
/// | 平台 | 手段 | 说明 |
/// |---|---|---|
/// | Windows | 注册表 `HardwareInformation.qwMemorySize` | WMI 的 `AdapterRAM` 是 u32，>4GB 会截断（实测 8GB 卡读成 4GB），只作兜底 |
/// | Linux | `/sys/class/drm/cardN/device/mem_info_vram_total` | AMD / Intel 走 sysfs；NVIDIA 闭源驱动不暴露该文件，回退 nvidia-smi |
/// | macOS | 引擎 `--list-devices` 或 nvidia-smi | 统一内存没有"独立显存"概念，且系统接口需 Metal 框架依赖，不值当 |
pub fn total_vram_mb() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        query_windows_total_vram()
    }
    #[cfg(target_os = "linux")]
    {
        query_linux_total_vram()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        // macOS：没有 sysfs/注册表；Metal 需要额外依赖。
        // 返回 None，上层退化成"不显示显存建议"，而不是给错数字。
        None
    }
}

/// Windows：读注册表里驱动写下的显存大小。
///
/// 走 PowerShell 而不是引入 `windows` crate：后者为一个数字加进来
/// 就是几百 KB，而我们**已经在用** PowerShell（查磁盘空间、查进程）。
///
/// 只读：`Get-ItemProperty` 不写、不建、不删任何东西。
#[cfg(target_os = "windows")]
fn query_windows_total_vram() -> Option<u64> {
    // 显卡类 GUID；子键里 DriverDesc + HardwareInformation.qwMemorySize
    const SCRIPT: &str = r#"
$ErrorActionPreference='SilentlyContinue'
$vc = @(Get-CimInstance Win32_VideoController)
$present = @($vc | ForEach-Object { ([string]$_.Name).Trim().ToLower() })
$best = 0
Get-ChildItem 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}' |
  ForEach-Object {
    $p = Get-ItemProperty $_.PSPath
    $desc = if ($p) { ([string]$p.DriverDesc).Trim() } else { '' }
    if (-not $desc) { return }
    if ($present.Count -gt 0 -and ($present -notcontains $desc.ToLower())) { return }
    $mem = $p.'HardwareInformation.qwMemorySize'
    if (-not $mem) {
      $m = $p.'HardwareInformation.MemorySize'
      if ($m -is [byte[]]) { $mem = [BitConverter]::ToUInt32($m, 0) } else { $mem = $m }
    }
    if ($mem -and [uint64]$mem -gt $best) { $best = [uint64]$mem }
  }
if ($best -gt 0) { [math]::Round($best / 1MB) }
"#;

    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT]);
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|mb| *mb > 0)
}

/// Linux：总显存来自 sysfs，NVIDIA 回退 nvidia-smi。
///
/// ## 只认 `cardN` 和 `renderD*`
///
/// `/sys/class/drm/card*` 通配会命中 `card0-Virtual-1` 这类**连接器**目录
/// （Kali 实测有 card0-Virtual-1..8），它们下面没有 `device/`。
/// 连接器不是显卡，扫进去只会得到一堆空值。
///
/// ## AMD / Intel 与 NVIDIA 的区别
///
/// `mem_info_vram_total` 由 amdgpu / i915 驱动提供；NVIDIA 闭源驱动没有
/// 这个文件，只能用 `nvidia-smi`。两者都有时取较大值（离散卡优先）。
#[cfg(target_os = "linux")]
fn query_linux_total_vram() -> Option<u64> {
    linux_total_vram_from(Path::new("/sys/class/drm")).or_else(query_nvidia_smi_total)
}

/// [`query_linux_total_vram`] 的核心，sysfs 根目录可替换。
///
/// ## 为什么把路径做成参数
///
/// 真实路径 `/sys/class/drm` 只在有 AMD / Intel 显卡的机器上才有
/// `mem_info_vram_total`。开发机是 N 卡（走 nvidia-smi 分支）、
/// 测试虚拟机是 VMware 虚拟显卡（vendor `0x15ad`，没有这个文件），
/// **两处都覆盖不到这条分支**。
///
/// 改成接受根目录后，可以拿一棵假的 sysfs 树把解析逻辑完整测一遍：
/// 连接器目录要跳过、`renderD*` 要认、空文件要忽略、多卡取最大。
/// 这些才是真正会写错的地方，而它们与"这台机器有没有 A 卡"无关。
///
/// 不加 ：测试要在任何平台跑得起来，
/// 否则这条分支永远只能靠一台有 A 卡的 Linux 机器来验。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_total_vram_from(drm_root: &Path) -> Option<u64> {
    let mut best: u64 = 0;

    let entries = std::fs::read_dir(drm_root).ok()?;
    for e in entries.flatten() {
        let Some(name) = e.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };
        // 只要 cardN（纯数字结尾）与 renderD*。
        // `card*` 通配会命中 `card0-Virtual-1` 这类**连接器**目录
        // （Kali 实测有 card0-Virtual-1..8），它们下面没有 `device/`。
        let is_card = name
            .strip_prefix("card")
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if !is_card && !name.starts_with("renderD") {
            continue;
        }
        let dev = e.path().join("device");
        let Ok(raw) = std::fs::read_to_string(dev.join("mem_info_vram_total")) else {
            continue;
        };
        if let Ok(bytes) = raw.trim().parse::<u64>() {
            let mb = bytes / (1024 * 1024);
            if mb > best {
                best = mb;
            }
        }
    }

    (best > 0).then_some(best)
}

/// `nvidia-smi` 读总显存（MB）。
///
/// 只在 Linux 用到：Windows 走注册表，那个值比 nvidia-smi 的
/// `memory.total` 更接近标称容量（后者会扣掉保留区）。
#[cfg(target_os = "linux")]
fn query_nvidia_smi_total() -> Option<u64> {
    let mut cmd = Command::new("nvidia-smi");
    cmd.args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"]);

    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .parse::<u64>()
        .ok()
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

    /// 用一棵**假的 sysfs 树**验证 Linux 显存解析。
    ///
    /// ## 为什么这样做
    ///
    /// 真实路径只在有 AMD / Intel 显卡的机器上才有
    /// `mem_info_vram_total`。开发机是 N 卡（走 nvidia-smi）、测试虚拟机
    /// 是 VMware 虚拟显卡（vendor `0x15ad`，没有这个文件）——
    /// 两处都覆盖不到这条分支。
    ///
    /// 而这段代码真正容易写错的地方与"有没有 A 卡"无关：
    ///
    /// - `card*` 通配会命中 `card0-Virtual-1` 这类**连接器**目录
    /// - 多卡时要取最大而不是累加
    /// - 空文件 / 非数字要跳过而不是崩
    ///
    /// 这些都能用假目录完整覆盖。真实 A 卡数据只有一份（RX 6750 GRE
    /// 10GB，注册表 10720641024 字节），作为第 4 条断言固化进来。
    #[test]
    fn linux_vram_reads_sysfs_correctly() {
        let root = std::env::temp_dir().join("xieyi-fake-drm");
        let _ = std::fs::remove_dir_all(&root);

        // 一个真实形状的 AMD 卡目录（10GB，取自 RX 6750 GRE 实测值）
        let card = root.join("card0/device");
        std::fs::create_dir_all(&card).unwrap();
        std::fs::write(card.join("mem_info_vram_total"), b"10720641024\n").unwrap();
        // 连接器目录：`card*` 通配会命中它，但它没有 device/
        // 连接器目录：`card*` 通配会命中它。
        //
        // **必须也放一个 device/mem_info_vram_total**，否则过滤器删掉
        // 测试照样过——因为本来就读不到文件。放一个更大的值才能区分：
        // 不过滤就会把它选成 best。
        let conn = root.join("card0-Virtual-1/device");
        std::fs::create_dir_all(&conn).unwrap();
        std::fs::write(conn.join("mem_info_vram_total"), b"34359738368").unwrap(); // 32GB
        std::fs::create_dir_all(root.join("card0-Virtual-2")).unwrap();

        let got = linux_total_vram_from(&root).expect("应当读到显存");
        assert_eq!(got, 10720641024 / (1024 * 1024), "10GB 卡应读成 10220 MB");

        // 再加一张更大的卡 → 取最大，不累加
        let card2 = root.join("card1/device");
        std::fs::create_dir_all(&card2).unwrap();
        std::fs::write(card2.join("mem_info_vram_total"), b"17179869184").unwrap(); // 16GB
        assert_eq!(
            linux_total_vram_from(&root),
            Some(17179869184 / (1024 * 1024)),
            "多卡取最大"
        );

        // 空文件 / 非数字：跳过而不是崩
        std::fs::write(card.join("mem_info_vram_total"), b"").unwrap();
        std::fs::write(card2.join("mem_info_vram_total"), b"nonsense").unwrap();
        assert_eq!(
            linux_total_vram_from(&root),
            None,
            "读不出数字就当没有，不能返回 0 或崩"
        );

        // 目录根本不存在 → None（Kali / 无 drm 的环境）
        assert_eq!(linux_total_vram_from(&root.join("nope")), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// `renderD*` 也要认：某些驱动只在这里暴露 mem_info。
    #[test]
    fn linux_vram_accepts_render_node() {
        let root = std::env::temp_dir().join("xieyi-fake-drm-render");
        let _ = std::fs::remove_dir_all(&root);
        let dev = root.join("renderD128/device");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("mem_info_vram_total"), b"8589934592").unwrap(); // 8GB

        assert_eq!(
            linux_total_vram_from(&root),
            Some(8589934592 / (1024 * 1024))
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 真机探测必须给出一个合理的显存值。
    ///
    /// 这条是**集成测试**（真的去问系统），不是单元测试。只验证「拿到的
    /// 数字落在合理范围」——具体数值取决于这台机器装了什么卡，写死会
    /// 立刻过期。
    ///
    /// 为什么值得保留：探针涉及注册表 / sysfs / 外部命令，全都可能因为
    /// 权限、驱动、输出格式变化而静默返回 `None`。这条能在环境变化时
    /// 立刻暴露。
    ///
    /// 无显卡的机器（CI、虚拟机）会返回 `None`，这时跳过而不是失败。
    #[test]
    fn total_vram_probe_gives_a_plausible_number() {
        if let Some(mb) = total_vram_mb() {
            assert!(mb >= 256, "总显存 {mb} MB 太小，探测多半读错了字段");
            assert!(
                mb <= 1024 * 1024,
                "总显存 {mb} MB 超过 1 TB，多半把字节当成 MB 了"
            );
        }
    }

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
