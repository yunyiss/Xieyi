import { useCallback, useEffect, useState } from "react";

/**
 * 主题偏好：跟随系统 / 强制浅色 / 强制深色。
 *
 * 存 localStorage 而不是后端配置：它纯粹是界面偏好，
 * 换台机器重设一次也无所谓，没必要占配置文件一行。
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

/** 把偏好写到 <html data-theme> 上；auto 时清掉属性，交给 CSS 的媒体查询。 */
function apply(mode: ThemeMode) {
  const root = document.documentElement;
  if (mode === "auto") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", mode);
}

export function useTheme() {
  const [mode, setMode] = useState<ThemeMode>(readStored);

  useEffect(() => {
    apply(mode);
    try {
      if (mode === "auto") localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, mode);
    } catch {
      // 存不了就算了，不影响当前会话
    }
  }, [mode]);

  const cycle = useCallback(() => {
    setMode((m) => (m === "auto" ? "light" : m === "light" ? "dark" : "auto"));
  }, []);

  return { mode, setMode, cycle };
}
