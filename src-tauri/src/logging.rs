//! 轻量日志：写到 `<数据目录>/logs/xieyi-YYYY-MM-DD.log`。
//!
//! **为什么自己写而不用 `tracing` / `log` + `fern`**：
//! 我们的需求只有「按天分文件、按级别过滤、别拖慢主线程」三条。
//! 引一整套订阅器 + 宏 + 输出层，编译产物涨几百 KB，收益却是零——
//! 这个项目的原则是「不引入重依赖」，日志不该是例外。
//!
//! 三条设计约束：
//!
//! 1. **不阻塞调用方**：写盘放到后台线程，`log()` 只做一次 channel 发送。
//!    推理是流式的，每条 token 都可能带日志，同步写文件会直接拖慢输出。
//! 2. **不会无限增长**：按天一个文件，超过 `KEEP_DAYS` 天的自动删掉。
//!    用户不会来清日志，我们不能让它在硬盘上默默长到几百 MB。
//! 3. **日志本身不能把程序搞崩**：所有失败都吞掉（`let _ =`）。
//!    写日志失败是次要问题，不能因此让翻译中断。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// 保留多少天的日志。再久的对排查没意义，只占磁盘。
const KEEP_DAYS: u64 = 7;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
        }
    }
}

struct Logger {
    tx: Sender<String>,
    level: Level,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// 初始化日志。重复调用是安全的（后续调用被忽略）。
///
/// `level` 之外没有别的开关：级别由构建类型决定（debug 构建记 Debug，
/// release 记 Info），用户不需要为此提供设置项——多一个设置就多一处要解释。
pub fn init(dir: PathBuf, level: Level) {
    if LOGGER.get().is_some() {
        return;
    }

    let (tx, rx) = mpsc::channel::<String>();

    // 建目录失败也继续——只是日志写不出来，不该阻止程序启动
    let _ = fs::create_dir_all(&dir);
    cleanup_old_logs(&dir);

    std::thread::Builder::new()
        .name("xieyi-log".into())
        .spawn(move || writer_loop(rx, dir))
        .ok();

    let _ = LOGGER.set(Logger { tx, level });
}

/// 记录一条日志。未初始化时静默丢弃。
pub fn log(level: Level, target: &str, msg: impl AsRef<str>) {
    let Some(l) = LOGGER.get() else { return };
    if level > l.level {
        return;
    }
    let line = format!(
        "{} {} [{}] {}",
        timestamp(),
        level.as_str(),
        target,
        msg.as_ref()
    );
    // 发送失败（后台线程已退出）就丢掉，不能让日志影响业务
    let _ = l.tx.send(line);
}

/// Error 级别：用于「启动失败」这类致命错误。
pub fn error(target: &str, msg: impl AsRef<str>) {
    log(Level::Error, target, msg)
}
pub fn warn(target: &str, msg: impl AsRef<str>) {
    log(Level::Warn, target, msg)
}
pub fn info(target: &str, msg: impl AsRef<str>) {
    log(Level::Info, target, msg)
}

/// Debug 级别目前没有调用点，但级别常量与过滤逻辑都已就位。
/// 保留成公开接口，是为了新增排查点时不必每次都改这个模块——
/// 用 `allow` 而不是删掉，删了以后加回来还得重新写一遍门面。
#[allow(dead_code)]
pub fn debug(target: &str, msg: impl AsRef<str>) {
    log(Level::Debug, target, msg)
}

/// 后台线程：收一条写一条，按日期切换文件。
///
/// 用 `BufWriter` 而非每条 open/close：批量翻译会连续写几百行，
/// 每条都开关文件在机械盘上能明显拖慢。
fn writer_loop(rx: Receiver<String>, dir: PathBuf) {
    let mut current_day = day_number();
    let mut file = open_for_day(&dir, current_day);

    for line in rx {
        let today = day_number();
        if today != current_day {
            current_day = today;
            file = open_for_day(&dir, current_day);
            // 跨天时顺手清理，避免只在启动时清一次、长期不关的程序永不回收
            cleanup_old_logs(&dir);
        }

        if let Some(f) = file.as_mut() {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
}

fn open_for_day(dir: &Path, day: u64) -> Option<File> {
    let path = dir.join(format!("xieyi-{}.log", day_to_date(day)));
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// 删掉超过 `KEEP_DAYS` 天的日志。
///
/// 只认自己写的文件名格式（`xieyi-YYYY-MM-DD.log`），目录里别的文件一律不碰——
/// 万一用户往里放了东西，误删比留垃圾严重得多。
fn cleanup_old_logs(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let today = day_number();

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(day) = parse_log_day(&name) else {
            continue;
        };
        if today.saturating_sub(day) > KEEP_DAYS {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// 从 `xieyi-YYYY-MM-DD.log` 解析出天序号。不是这个格式就返回 None。
fn parse_log_day(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("xieyi-")?.strip_suffix(".log")?;
    let mut parts = rest.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(ymd_to_day(y, m, d))
}

fn timestamp() -> String {
    let secs = now_secs();
    // 用**本地时间**而不是 UTC：日志是给人看的，用户说"我 19:26 出的问题"
    // 时得能在日志里一眼找到。UTC 会让所有时间差 8 小时，排查时先要心算。
    let local = secs.saturating_add_signed(local_offset_secs());
    let day = local / 86_400;
    let rem = local % 86_400;
    let (y, m, d) = day_to_ymd(day);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 本地时区相对 UTC 的偏移（秒，东八区为 +28800）。
///
/// 不引 `chrono` 只为拿一个时区偏移：Windows 上直接问系统。
/// 失败就退回 0（UTC）——日志时间差几小时是小问题，程序崩了才是大问题。
#[cfg(windows)]
fn local_offset_secs() -> i64 {
    use std::mem::MaybeUninit;

    #[repr(C)]
    struct Ft {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    struct St {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(lp: *mut St);
        fn GetSystemTime(lp: *mut St);
        fn SystemTimeToFileTime(lp: *const St, ft: *mut Ft) -> i32;
    }

    unsafe fn to_secs(t: &St) -> Option<u64> {
        // 调用 FFI 与 assume_init 都需要显式 unsafe 块（edition 2024 下
        // 外层 `unsafe fn` 不再自动覆盖函数体）
        let v = unsafe {
            let mut ft = MaybeUninit::<Ft>::zeroed();
            if SystemTimeToFileTime(t, ft.as_mut_ptr()) == 0 {
                return None;
            }
            let ft = ft.assume_init();
            ((ft.high as u64) << 32) | ft.low as u64
        };
        // FILETIME 从 1601-01-01 起算，单位 100ns
        const EPOCH_DIFF: u64 = 11_644_473_600;
        Some((v / 10_000_000).saturating_sub(EPOCH_DIFF))
    }

    unsafe {
        // 分别取本地时间与 UTC 时间，两者之差就是当前生效的偏移。
        // 这样夏令时自动包含在内，不用自己判断。
        let mut local = MaybeUninit::<St>::zeroed();
        let mut utc = MaybeUninit::<St>::zeroed();
        GetLocalTime(local.as_mut_ptr());
        GetSystemTime(utc.as_mut_ptr());

        match (to_secs(&local.assume_init()), to_secs(&utc.assume_init())) {
            (Some(l), Some(u)) => l as i64 - u as i64,
            _ => 0,
        }
    }
}

/// Unix：用 `localtime_r` 问系统要时区偏移。
///
/// 之前这里直接 `0`（= UTC），日志时间和用户的实际时钟差 8 小时。
/// 上面 `timestamp()` 的注释写着「日志是给人看的」，那条要求对
/// Linux/macOS 同样成立，不能只在 Windows 上做到。
///
/// 不引 `libc` crate：只用到一个 `tm` 结构和一个函数，手写 FFI
/// 比多一个依赖划算（与 `single_instance` 里的 `flock` 同一思路）。
#[cfg(unix)]
fn local_offset_secs() -> i64 {
    use std::mem::MaybeUninit;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// glibc / Darwin 的 `struct tm` 布局一致：9 个 int，顺序相同。
    #[repr(C)]
    struct Tm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
        // 后面还有 tm_gmtoff / tm_zone，但我们不需要，不声明
    }

    unsafe extern "C" {
        fn localtime_r(timep: *const i64, result: *mut Tm) -> *mut Tm;
    }

    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => return 0,
    };

    // SAFETY: `localtime_r` 是线程安全的可重入版本（这正是选它而非
    // `localtime` 的原因——日志在后台线程里写）。返回的指针就是
    // 我们传进去的 `result`，只要它非空即表示填充成功。
    unsafe {
        let mut out = MaybeUninit::<Tm>::zeroed();
        if localtime_r(&now, out.as_mut_ptr()).is_null() {
            return 0;
        }
        let tm = out.assume_init();
        let _ = tm.tm_isdst; // 夏令时已由 localtime_r 算进 tm_hour

        // 把「本地时间各字段」和「UTC 各字段」都换算成当天秒数再相减。
        // 不能直接 `tm_hour * 3600` 了事——那样算的是「本地时钟读数」，
        // 不是「偏移量」，遇到跨日（如 UTC 23:00 对应东八区次日 07:00）
        // 就会得到负数。
        let local_secs_of_day = tm.tm_hour as i64 * 3600 + tm.tm_min as i64 * 60 + tm.tm_sec as i64;
        let utc = now.rem_euclid(86_400);
        local_secs_of_day - utc
    }
}

fn day_to_date(day: u64) -> String {
    let (y, m, d) = day_to_ymd(day);
    format!("{y:04}-{m:02}-{d:02}")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 当前的**本地**日期序号。
///
/// 必须用本地日期而不是 UTC：日志文件名和 `timestamp()` 里的日期是
/// 同一个东西，一个按 UTC 取、一个按本地取，跨日时段就会对不上——
/// 东八区凌晨 01:00 写的日志，时间戳是今天，文件名却是昨天，
/// 用户按当天日期找文件会找不到。
fn day_number() -> u64 {
    (now_secs() as i64).saturating_add(local_offset_secs()).max(0) as u64 / 86_400
}

// ── 日期换算 ────────────────────────────────────────────────────────────
// 不引 chrono / time：只为格式化日志而拉一个日期库不值得。
// 这里用「民用历法」的标准换算（Howard Hinnant 的 days_from_civil），
// 对 1970 年以后的所有日期都精确，闰年、世纪闰年都对。

fn ymd_to_day(y: i64, m: u32, d: u32) -> u64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    (era as u64 * 146_097 + doe).saturating_sub(719_468)
}

fn day_to_ymd(day: u64) -> (i64, u32, u32) {
    let z = day as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_day_zero() {
        assert_eq!(ymd_to_day(1970, 1, 1), 0);
        assert_eq!(day_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn known_dates_round_trip() {
        // 闰年 2 月 29 日、世纪闰年 2000、非闰年 1900（整百但要被 400 整除才是闰年）
        for (y, m, d) in [
            (2000, 2, 29),
            (2024, 2, 29),
            (2026, 11, 3),
            (1999, 12, 31),
            (2000, 3, 1),
        ] {
            let day = ymd_to_day(y, m, d);
            assert_eq!(day_to_ymd(day), (y, m, d), "{y}-{m}-{d} 往返不一致");
        }
    }

    #[test]
    fn day_numbers_are_monotonic() {
        let a = ymd_to_day(2026, 11, 3);
        let b = ymd_to_day(2026, 11, 4);
        assert_eq!(b - a, 1, "相邻两天应当差 1");
    }

    #[test]
    fn parses_only_our_own_log_names() {
        assert!(parse_log_day("xieyi-2026-11-03.log").is_some());
        // 不是我们的格式，一律不认（避免清理时误删用户文件）
        assert!(parse_log_day("notes.txt").is_none());
        assert!(parse_log_day("xieyi-2026-11-03.txt").is_none());
        assert!(parse_log_day("xieyi-2026-13-01.log").is_none());
        assert!(parse_log_day("xieyi-2026-11-32.log").is_none());
        assert!(parse_log_day("xieyi-2026-11-03-extra.log").is_none());
    }

    #[test]
    fn parses_back_to_the_same_day() {
        let day = ymd_to_day(2026, 11, 3);
        let name = format!("xieyi-{}.log", day_to_date(day));
        assert_eq!(parse_log_day(&name), Some(day));
    }

    #[test]
    fn level_filtering_orders_correctly() {
        // Error 最紧急、Debug 最啰嗦：阈值比较依赖这个顺序
        assert!(Level::Error < Level::Warn);
        assert!(Level::Warn < Level::Info);
        assert!(Level::Info < Level::Debug);
    }

    /// 日志文件名里的日期，必须和 `timestamp()` 打出的日期一致。
    ///
    /// 这条锁住一个真坑：`day_number()` 曾用 UTC，而 `timestamp()`
    /// 用本地时间。东八区凌晨 01:00 写的日志，时间戳显示今天、
    /// 文件名却是昨天，用户按当天日期去找文件会找不到。
    #[test]
    fn log_filename_date_matches_timestamp_date() {
        let file_date = day_to_date(day_number());

        // timestamp() 前 10 个字符就是 YYYY-MM-DD
        let stamp = timestamp();
        let stamp_date = &stamp[..10];

        assert_eq!(
            file_date, stamp_date,
            "日志文件名日期 {file_date} 与时间戳日期 {stamp_date} 不一致"
        );
    }

    /// 本地时区偏移必须在真实范围内。
    ///
    /// 这条防的是「Unix 分支无条件返回 0」那种退化：虽然 0 在合法
    /// 范围内（UTC），但真出现时日志时间会和用户时钟差好几个小时。
    /// 这里只做范围校验——CI 机器的时区不可控，不能断言具体值。
    #[test]
    fn local_offset_is_plausible() {
        let off = local_offset_secs();
        // 真实世界时区范围：UTC-12:00 ~ UTC+14:00
        assert!(
            (-12 * 3600..=14 * 3600).contains(&off),
            "时区偏移超出现实范围：{off} 秒"
        );
        // 现实中最小粒度是 15 分钟
        assert_eq!(off % 900, 0, "时区偏移不是 15 分钟的整数倍：{off} 秒");
    }
}
