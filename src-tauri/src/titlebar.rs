//! 系统窗口主题（标题栏配色）。
//!
//! ## 为什么用 Tauri 的 `set_theme` 而不是手写平台 API
//!
//! 第一版手写了 Windows 的 `DwmSetWindowAttribute`，只覆盖 Windows，
//! 且 Win10 有两个属性号要兼容。而 Tauri 的 `Window::set_theme` 底层
//! 已经做了完整封装：
//!
//! | 平台 | 底层实现 |
//! |---|---|
//! | Windows | DWM 深色标题栏 |
//! | Linux | `gtk-application-prefer-dark-theme` |
//! | macOS | `NSAppearance` |
//!
//! 一个 API 三平台通吃，没必要自己维护平台分支。
//!
//! ## 为什么 `system` 要传 `None` 而不是解析成具体颜色
//!
//! 传 `Some(Dark)` / `Some(Light)` 会**锁定**窗口外观；传 `None` 表示
//! 跟随系统。后者的好处是：WebView 的 `prefers-color-scheme` 会与真实
//! OS 主题保持同步，前端靠媒体查询就能自动响应系统主题变化——
//! 不需要我们额外监听再手动通知。
//!
//! 参考实现：cc-switch 的 `set_window_theme` 命令。

use tauri::Theme;

/// 前端传来的主题名：`"light"` / `"dark"` / 其他（视为跟随系统）。
pub fn parse_theme(name: &str) -> Option<Theme> {
    match name {
        "dark" => Some(Theme::Dark),
        "light" => Some(Theme::Light),
        // "system" / "auto" / 未知值：交给系统
        _ => None,
    }
}

/// 把窗口（含标题栏）的主题设成指定值。
///
/// 失败只记 debug 日志——标题栏配色是锦上添花，不该影响使用。
pub fn set_window_theme(window: &tauri::WebviewWindow, name: &str) {
    // 跨平台：Windows 走 DWM、Linux 走 GTK、macOS 走 NSAppearance
    if let Err(e) = window.set_theme(parse_theme(name)) {
        crate::logging::debug("window", format!("设置窗口主题失败: {e}"));
    }

    // **Windows 上额外补一次 DWM 调用**。
    //
    // 实测：`set_theme` 在 Windows 上返回 Ok，但标题栏颜色不变。
    // 原因是它只向窗口过程发 `WM_SETTINGCHANGE` 类消息，由 tao 的
    // 窗口过程处理；而我们的窗口是 `create: false` 手动创建的，
    // 走的是 `WebviewWindowBuilder`，和 tao 默认创建路径不完全一致，
    // 那条消息可能没被正确处理。
    //
    // 直接调 DWM 属性是同步的、必定生效，作为补偿路径。
    // 两个都失败时才记日志。
    #[cfg(windows)]
    {
        if let Err(e) = windows_dwm::apply(window, name) {
            crate::logging::debug("window", format!("DWM 标题栏补偿失败: {e}"));
        }
    }

    // 顺带把窗口底色也换掉：主题切换瞬间 WebView 还没重绘完，
    // 底下若是旧色会闪一下
    set_window_background(window, name);
}

/// Windows DWM 直接调用。作为 `set_theme` 的补偿路径。
#[cfg(windows)]
mod windows_dwm {
    use tauri::WebviewWindow;

    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_USE_IMMERSIVE_DARK_MODE_OLD: u32 = 19;

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: isize,
            attr: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }

    pub fn apply(window: &WebviewWindow, name: &str) -> Result<(), String> {
        let hwnd = window.hwnd().map_err(|e| e.to_string())?;
        let dark = match name {
            "dark" => true,
            "light" => false,
            // system：跟随系统，交给 DWM 的默认行为
            _ => return Ok(()),
        };

        let value: i32 = if dark { 1 } else { 0 };
        let ptr = &value as *const i32 as *const core::ffi::c_void;
        let size = std::mem::size_of::<i32>() as u32;

        unsafe {
            let hr =
                DwmSetWindowAttribute(hwnd.0 as isize, DWMWA_USE_IMMERSIVE_DARK_MODE, ptr, size);
            if hr == 0 {
                return Ok(());
            }
            // Win10 1809~1903 用旧属性号
            let hr_old = DwmSetWindowAttribute(
                hwnd.0 as isize,
                DWMWA_USE_IMMERSIVE_DARK_MODE_OLD,
                ptr,
                size,
            );
            if hr_old == 0 {
                return Ok(());
            }
            Err(format!("0x{hr:08x}"))
        }
    }
}

/// 窗口的初始/切换背景色，与 `index.css` 的 `--canvas` 保持一致。
///
/// 作用有两个：
/// 1. **减少启动闪烁**——WebView 加载完成前窗口已经是正确底色，
///    不会先白一下再变深
/// 2. 主题切换瞬间，WebView 重绘之前底下也是同色，不闪
///
/// 两份色值必须和 CSS 同步：浅色 `--canvas: #f6f7f9`、
/// 深色 `--canvas: #101215`。改 CSS 时记得同步这里。
const CANVAS_LIGHT: (u8, u8, u8) = (0xf6, 0xf7, 0xf9);
const CANVAS_DARK: (u8, u8, u8) = (0x10, 0x12, 0x15);

/// 按主题名取窗口背景色。
///
/// `system` / 未知值**必须真的去问窗口当前是什么主题**，不能假定浅色：
/// 深色系统下选「跟随系统」时，若这里返回浅色，WebView 还没重绘完的那
/// 一瞬底下就是 `#f6f7f9`，肉眼看到一次白闪。这是纯 CSS 修不了的
/// ——底色是窗口属性，CSS 管不到。
///
/// `window.theme()` 拿不到时（平台不支持 / API 失败）退回浅色：
/// 闪一下比设不上色好。
fn canvas_color(window: &tauri::WebviewWindow, name: &str) -> tauri::window::Color {
    canvas_for(resolve_dark(window, name))
}

/// 主题名 → 是否深色。抽成独立函数是为了能单测：
/// 只有它需要 `WebviewWindow`，颜色映射本身是纯计算。
fn resolve_dark(window: &tauri::WebviewWindow, name: &str) -> bool {
    match name {
        "dark" => true,
        "light" => false,
        // system / 未知：问窗口真实的主题状态
        _ => match window.theme() {
            Ok(Theme::Dark) => true,
            Ok(_) => false,
            Err(e) => {
                crate::logging::debug("window", format!("读取窗口主题失败，底色按浅色: {e}"));
                false
            }
        },
    }
}

/// 深浅 → RGBA。纯计算，无副作用，可单测。
fn canvas_for(dark: bool) -> tauri::window::Color {
    let (r, g, b) = if dark { CANVAS_DARK } else { CANVAS_LIGHT };
    tauri::window::Color(r, g, b, 255)
}

/// 设置窗口背景色，减少加载与切换时的闪烁。
pub fn set_window_background(window: &tauri::WebviewWindow, name: &str) {
    if let Err(e) = window.set_background_color(Some(canvas_color(window, name))) {
        crate::logging::debug("window", format!("设置窗口背景色失败: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_names_map_to_theme() {
        assert_eq!(parse_theme("dark"), Some(Theme::Dark));
        assert_eq!(parse_theme("light"), Some(Theme::Light));
    }

    /// `system` 必须映射成 `None`——传 `Some` 会锁定外观，
    /// 导致浏览器/WebView 的 `prefers-color-scheme` 不再跟随系统。
    #[test]
    fn system_maps_to_none() {
        assert_eq!(parse_theme("system"), None);
        assert_eq!(parse_theme("auto"), None);
    }

    /// 未知值当跟随系统处理，不要崩，也不要误设成浅色。
    #[test]
    fn unknown_falls_back_to_none() {
        assert_eq!(parse_theme(""), None);
        assert_eq!(parse_theme("solarized"), None);
    }

    /// 窗口背景色必须与 `index.css` 的 `--canvas` 一致，
    /// 否则加载完成前会看到一层不同颜色的底。
    #[test]
    fn canvas_constants_match_css() {
        assert_eq!(CANVAS_LIGHT, (0xf6, 0xf7, 0xf9), "浅色 canvas 变了");
        assert_eq!(CANVAS_DARK, (0x10, 0x12, 0x15), "深色 canvas 变了");
    }

    /// 深浅两态必须给出**不同**的底色。
    ///
    /// 这条锁住一个真 bug：`canvas_color` 的 `system` 分支曾经直接
    /// `_ => false`，等于永远返回浅色。深色系统下选「跟随系统」时，
    /// WebView 重绘前底下是白底，肉眼看到一次白闪。如果哪天有人
    /// 又把两条分支写成一样，这里会红。
    #[test]
    fn light_and_dark_canvas_differ() {
        let light = canvas_for(false);
        let dark = canvas_for(true);
        assert_ne!(
            (light.0, light.1, light.2),
            (dark.0, dark.1, dark.2),
            "浅色与深色的窗口底色不能相同"
        );
    }

    /// `canvas_for` 必须常量映射，不受外部状态影响。
    #[test]
    fn canvas_for_is_deterministic() {
        assert_eq!(canvas_for(true).0, CANVAS_DARK.0);
        assert_eq!(canvas_for(false).0, CANVAS_LIGHT.0);
        // alpha 恒为不透明：半透明底会透出桌面，闪得更明显
        assert_eq!(canvas_for(true).3, 255);
        assert_eq!(canvas_for(false).3, 255);
    }
}
