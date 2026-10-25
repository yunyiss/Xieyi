import { useCallback, useEffect, useState } from "react";
import {
  getSettings,
  listLocalModels,
  listRouterModels,
  restartService,
  setGpuLayers,
  setPinned,
  setTtl,
  unloadModel,
  type LocalModel,
  type Settings,
} from "../lib/ipc";

interface Props {
  onChanged: () => void;
}

const cardStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
};

const controlClass =
  "h-8 rounded-lg border px-2 text-[12px] outline-none focus:border-[var(--brand)]";

const controlStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

const TTL_OPTIONS = [
  { value: 60, label: "60 秒后释放" },
  { value: 120, label: "120 秒后释放" },
  { value: 300, label: "5 分钟后释放" },
  { value: 0, label: "不自动释放" },
];

const LAYER_OPTIONS = [
  { value: 999, label: "全 GPU" },
  { value: 16, label: "混合（16 层）" },
  { value: 0, label: "纯 CPU" },
];

export function ModelPanel({ onChanged }: Props) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [local, setLocal] = useState<LocalModel[]>([]);
  const [states, setStates] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");

  const refresh = useCallback(async () => {
    try {
      const s = await getSettings();
      const files = await listLocalModels();
      setSettings(s);
      setLocal(files);

      try {
        const router = await listRouterModels();
        const map: Record<string, string> = {};
        router.forEach((m) => (map[m.id] = m.state));
        setStates(map);
      } catch {
        setStates({});
      }
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const togglePin = async (id: string, next: boolean) => {
    setError("");
    try {
      const pinned = await setPinned(id, next);
      setSettings((prev) => (prev ? { ...prev, pinned } : prev));
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

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

  const restart = async () => {
    setError("");
    setBusy("restart");
    try {
      await restartService();
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const unload = async (id: string) => {
    setError("");
    setBusy(id);
    try {
      await unloadModel(id);
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const pinned = settings?.pinned ?? [];

  return (
    <section
      className="rounded-xl border p-3"
      style={cardStyle}
    >
      {/* 设置行 */}
      <div className="mb-3 flex flex-wrap items-center gap-3">
        <label className="flex items-center gap-1.5 text-[12px]">
          <span style={{ color: "var(--text-muted)" }}>空闲释放</span>
          <select
            className={controlClass}
            style={controlStyle}
            value={settings?.ttlSecs ?? 120}
            onChange={(e) => changeTtl(Number(e.target.value))}
          >
            {TTL_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>

        <label className="flex items-center gap-1.5 text-[12px]">
          <span style={{ color: "var(--text-muted)" }}>推理档位</span>
          <select
            className={controlClass}
            style={controlStyle}
            value={settings?.gpuLayers ?? 999}
            onChange={(e) => changeLayers(Number(e.target.value))}
          >
            {LAYER_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </label>

        <button
          className="h-8 rounded-lg border px-3 text-[12px] disabled:opacity-50"
          style={controlStyle}
          onClick={restart}
          disabled={busy === "restart"}
        >
          {busy === "restart" ? "重启中…" : "重启引擎"}
        </button>

        <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
          档位改动需重启引擎生效
        </span>
      </div>

      {/* 模型卡片 */}
      <div className="grid grid-cols-1 gap-2 md:grid-cols-2 xl:grid-cols-3">
        {local.length === 0 && (
          <p className="m-0 text-[12px]" style={{ color: "var(--text-muted)" }}>
            模型目录里还没有 .gguf 文件。
          </p>
        )}

        {local.map((m) => {
          const state = states[m.id] ?? "unloaded";
          const isPinned = pinned.includes(m.id);
          const isLoaded = state === "loaded" || state === "loading";

          return (
            <article
              key={m.id}
              className="flex flex-col gap-1 rounded-lg border p-2.5"
              style={{
                background: "var(--surface-muted)",
                borderColor: isPinned
                  ? "color-mix(in srgb, var(--brand) 55%, var(--border))"
                  : "var(--border)",
              }}
            >
              <div className="flex items-center gap-2">
                <span className="truncate text-[13px] font-medium" title={m.id}>
                  {m.id}
                </span>
                <span
                  className="ml-auto shrink-0 rounded-full px-1.5 py-0.5 text-[10px]"
                  style={{
                    background: isLoaded ? "var(--brand-soft)" : "transparent",
                    color: isLoaded ? "var(--brand-text)" : "var(--text-muted)",
                    border: isLoaded ? "none" : "1px solid var(--border)",
                  }}
                >
                  {state === "loaded" ? "已加载" : state === "loading" ? "加载中" : "冷"}
                </span>
              </div>

              <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
                {m.sizeMb} MB
              </span>

              <div className="mt-1 flex items-center gap-2">
                <label className="flex cursor-pointer items-center gap-1.5 text-[11px]">
                  <input
                    type="checkbox"
                    checked={isPinned}
                    onChange={(e) => togglePin(m.id, e.target.checked)}
                    className="accent-[var(--brand)]"
                  />
                  常驻
                </label>

                {isLoaded && (
                  <button
                    className="ml-auto h-6 rounded-md border px-2 text-[11px]"
                    style={controlStyle}
                    onClick={() => unload(m.id)}
                    disabled={busy === m.id}
                  >
                    {busy === m.id ? "卸载中…" : "卸载"}
                  </button>
                )}
              </div>
            </article>
          );
        })}
      </div>

      {error && (
        <p className="m-0 mt-2 text-[12px]" style={{ color: "var(--danger)" }}>
          {error}
        </p>
      )}
    </section>
  );
}
