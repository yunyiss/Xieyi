use std::process::Command;

/// 查询当前空闲显存（MB）。优先用 nvidia-smi，没有 NVIDIA 显卡时返回 None。
pub fn free_vram_mb() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let output = Command::new("nvidia-smi")
            .args([
                "--query-gpu=memory.free",
                "--format=csv,noheader,nounits",
            ])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output()
            .ok()?;

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

    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}
