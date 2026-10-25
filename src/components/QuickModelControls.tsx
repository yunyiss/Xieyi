import { useCallback, useEffect, useState } from "react";
import {
  getSettings,
  restartService,
  setGpuLayers,
  setPinned,
  setTtl,
  type Settings,
} from "../lib/ipc";

interface Props {
  model: string;
  onChanged: () => void;
  onOpenDrawer: () => void;
}

const controlClass =
  "h-8 rounded-lg border px-2 text-[12px] outline-none focus:border-[var(--brand)]";

const controlStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

const TTL_OPTIONS = [
  { value: 60, label: "空闲 60 秒释放" },
  { value: 120, label: "空闲 120 秒释放" },
  { value: 300, label: "空闲 5 分钟释放" },
  { value: 0, label: "不自动释放" },
];

const LAYER_OPTIONS = [
  { value: 999, label: "全 GPU" },
  { value: 16, label: "混合 16 层" },
  { value: 0, label: "纯 CPU" },
];

/** 常驻控制：放在主界面一行的快捷设置。 */
export function QuickModelControls({ model, onChanged, onOpenDrawer }: Props) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    try {
      setSettings(await getSettings());
    } catch {
      // 引擎未启用时忽略
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const pinned = settings?.pinned ?? [];
  const isPinned = Boolean(model && pinned.includes(model));

  const changeTtl = async (value: number) => {
    setError("");
    try {
      await setTtl(value);
      setSettings((prev) => (prev ? { ...prev, ttlSecs: value } : prev));
    } catch (e) {
      setError(String(e));
    }
  };

  const changeLayers = async (value: number) => {
    setError("");
    try {
      await setGpuLayers(value);
      setSettings((prev) => (prev ? { ...prev, gpuLayers: value } : prev));
    } catch (e) {
      setError(String(e));
    }
  };

  const togglePin = async (next: boolean) => {
    if (!model) return;
    setError("");
    try {
      const list = await setPinned(model, next);
      setSettings((prev) => (prev ? { ...prev, pinned: list } : prev));
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const restart = async () => {
    setError("");
    try {
      await restartService();
      onChanged();
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="flex flex-wrap items-center gap-2">
      <select
        className={controlClass}
        style={controlStyle}
        value={settings?.ttlSecs ?? 120}
        onChange={(e) => changeTtl(Number(e.target.value))}
        title="翻译/对话结束后多久释放显存"
      >
        {TTL_OPTIONS.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>

      <select
        className={controlClass}
        style={controlStyle}
        value={settings?.gpuLayers ?? 999}
        onChange={(e) => changeLayers(Number(e.target.value))}
        title="推理档位，改动后需重启引擎"
      >
        {LAYER_OPTIONS.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>

      <label className="flex cursor-pointer items-center gap-1.5 text-[12px]">
        <input
          type="checkbox"
          checked={isPinned}
          disabled={!model}
          onChange={(e) => togglePin(e.target.checked)}
          className="accent-[var(--brand)]"
        />
        常驻
      </label>

      <button
        className="h-8 rounded-lg border px-2.5 text-[12px]"
        style={controlStyle}
        onClick={restart}
        title="用当前档位重启引擎"
      >
        重启引擎
      </button>

      <button
        className="flex h-8 items-center gap-1 rounded-lg border px-2.5 text-[12px]"
        style={controlStyle}
        onClick={onOpenDrawer}
        title="模型管理"
      >
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden>
          <path
            d="M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Z"
            stroke="currentColor"
            strokeWidth="1.7"
          />
          <path
            d="M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1.03 1.56V21a2 2 0 1 1-4 0v-.09A1.7 1.7 0 0 0 8.9 19.4a1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.7 1.7 0 0 0 4.6 15a1.7 1.7 0 0 0-1.56-1.03H3a2 2 0 1 1 0-4h.09A1.7 1.7 0 0 0 4.6 8.9a1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.7 1.7 0 0 0 9 4.6h.09A1.7 1.7 0 0 0 10.1 3.04V3a2 2 0 1 1 4 0v.09a1.7 1.7 0 0 0 1.03 1.56 1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.7 1.7 0 0 0 19.4 9v.09a1.7 1.7 0 0 0 1.56 1.03H21a2 2 0 1 1 0 4h-.09A1.7 1.7 0 0 0 19.4 15Z"
            stroke="currentColor"
            strokeWidth="1.7"
            strokeLinejoin="round"
          />
        </svg>
        管理
      </button>

      {error && (
        <span className="text-[11px]" style={{ color: "var(--danger)" }}>
          {error}
        </span>
      )}
    </div>
  );
}
