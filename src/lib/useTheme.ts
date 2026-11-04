import { useEffect, useState } from "react";
import { syncWindowTheme } from "./ipc";

/**
 * 主题偏好：跟随系统 / 强制浅色 / 强制深色。
 *
 * 存 localStorage 而不是后端配置：它纯粹是界面偏好，
 * 换台机器重设一次也无所谓，没必要占配置文件一行。
 *
 * 只暴露 `setMode` 而不提供「循环切换」——三态循环要按顺序点，
 * 从「跟随系统」到「深色」得点两次，用户无法直达目标状态。
 * 交互交给 `ThemeMenu` 的下拉菜单，一次点到。
 */
export type ThemeMode = "auto" | "light" | "dark";

const KEY = "xieyi.theme";

function readStored(): ThemeMode {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "light" || v === "dark" || v === "auto") return v;
  } catch {
    // 隐私模式禁用 localStorage 时静默回退
  }
  return "auto";
}

/**
 * 把偏好写到 `<html data-theme>` 上；auto 时清掉属性，交给 CSS 的媒体查询。
 *
 * ## 为什么要临时禁用过渡
 *
 * 界面里大量元素带 `transition: background 110ms`。直接切主题的话，
 * **每个元素都会各自慢慢变色**，整屏像"一格一格刷过去"，观感很差。
 *
 * 做法（参考 cc-switch）：切换前加 `.theme-switching` 类禁用所有过渡，
 * 改完强制重排让新颜色立即生效，再等两帧恢复过渡。
 * 两帧是为了避开浏览器把"同一帧内的样式变更"合并掉的情况。
 */
function apply(mode: ThemeMode) {
  const root = document.documentElement;

  root.classList.add("theme-switching");
  if (mode === "auto") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", mode);

  // 强制重排：让不带过渡的新颜色先落定
  void document.body?.offsetHeight;

  requestAnimationFrame(() => {
    requestAnimationFrame(() => {
      root.classList.remove("theme-switching");
    });
  });
}

export function useTheme() {
  const [mode, setMode] = useState<ThemeMode>(readStored);

  useEffect(() => {
    apply(mode);
    syncWindowTheme(mode === "auto" ? "system" : mode);
    try {
      if (mode === "auto") localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, mode);
    } catch {
      // 存不了就算了，不影响当前会话
    }
  }, [mode]);

  // auto 模式下不需要自己监听系统主题变化：传给后端的是 `None`
  // （跟随系统），Tauri 会让窗口外观自然跟随，WebView 的
  // `prefers-color-scheme` 也保持一致，CSS 媒体查询自动生效。

  return { mode, setMode };
}
