import { useCallback, useEffect, useState } from "react";
import {
  abort,
  getGlossary,
  getSettings,
  listLocalModels,
  listRouterModels,
  openModelsDir,
  restartService,
  setGlossary,
  setGpuLayers,
  setModelsDir,
  setPinned,
  setTtl,
  type GlossaryEntry,
  type LocalModel,
  type Settings,
} from "../lib/ipc";

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

const fieldStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

const fieldClass =
  "h-8 w-full rounded-lg border px-2 text-[12px] outline-none focus:border-[var(--brand)]";

interface Props {
  open: boolean;
  onClose: () => void;
  onChanged: () => void;
}

const btnClass =
  "h-7 rounded-md border px-2 text-[11px] transition-colors disabled:opacity-50";

const btnStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

function stateLabel(state: string): { text: string; hot: boolean } {
  if (state === "loaded") return { text: "已加载", hot: true };
  if (state === "loading") return { text: "加载中", hot: true };
  if (state === "unloading") return { text: "卸载中", hot: true };
  return { text: "未加载", hot: false };
}

/** 右侧抽屉：模型列表与目录管理。 */
export function ModelDrawer({ open, onClose, onChanged }: Props) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [local, setLocal] = useState<LocalModel[]>([]);
  const [states, setStates] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [glossaryText, setGlossaryText] = useState("");
  const [glossarySaved, setGlossarySaved] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const s = await getSettings();
      setSettings(s);
      setLocal(await listLocalModels());

      const terms = await getGlossary();
      setGlossaryText(
        (prev) => prev || terms.map((t) => `${t.from}=${t.to}`).join("\n"),
      );

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
    if (!open) return;
    refresh();
    const timer = window.setInterval(refresh, 2500);
    return () => window.clearInterval(timer);
  }, [open, refresh]);

  if (!open) return null;

  const pinned = settings?.pinned ?? [];

  const togglePin = async (id: string, next: boolean) => {
    setError("");
    try {
      const list = await setPinned(id, next);
      setSettings((prev) => (prev ? { ...prev, pinned: list } : prev));
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const stopModel = async (id: string) => {
    setError("");
    setBusy(id);
    try {
      await abort(id);
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
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

  const changeDir = async () => {
    const dir = window.prompt(
      "输入模型目录（存放 .gguf 文件）：",
      settings?.modelsDir ?? "",
    );
    if (!dir) return;
    try {
      await setModelsDir(dir);
      await restartService();
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const saveGlossary = async () => {
    setError("");
    const entries: GlossaryEntry[] = glossaryText
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => {
        const at = line.indexOf("=");
        if (at <= 0) return null;
        return {
          from: line.slice(0, at).trim(),
          to: line.slice(at + 1).trim(),
        };
      })
      .filter((e): e is GlossaryEntry => Boolean(e && e.from && e.to));

    try {
      await setGlossary(entries);
      setGlossarySaved(true);
      window.setTimeout(() => setGlossarySaved(false), 1500);
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

  return (
    <>
      <div
        className="fixed inset-0 z-40"
        style={{ background: "rgba(0,0,0,0.28)" }}
        onClick={onClose}
      />
      <aside
        className="fixed right-0 top-0 z-50 flex h-full w-[380px] flex-col border-l"
        style={{
          background: "var(--surface)",
          borderColor: "var(--border)",
          maxWidth: "92vw",
        }}
      >
        <header
          className="flex shrink-0 items-center gap-2 border-b px-4 py-3"
          style={{ borderColor: "var(--border)" }}
        >
          <h2 className="m-0 text-[14px] font-medium">设置与模型</h2>
          <button
            className="ml-auto h-7 w-7 rounded-md border text-[13px]"
            style={btnStyle}
            onClick={onClose}
            aria-label="关闭"
          >
            ✕
          </button>
        </header>

        <div className="min-h-0 flex-1 overflow-auto px-4 py-3">
          {/* 引擎设置 */}
          <section className="mb-4">
            <div
              className="mb-1.5 text-[11px] font-medium"
              style={{ color: "var(--text-muted)" }}
            >
              引擎设置
            </div>

            <div className="flex flex-col gap-2">
              <label className="flex items-center gap-2 text-[12px]">
                <span className="w-[64px] shrink-0" style={{ color: "var(--text-muted)" }}>
                  空闲释放
                </span>
                <select
                  className={fieldClass}
                  style={fieldStyle}
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

              <label className="flex items-center gap-2 text-[12px]">
                <span className="w-[64px] shrink-0" style={{ color: "var(--text-muted)" }}>
                  推理档位
                </span>
                <select
                  className={fieldClass}
                  style={fieldStyle}
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
            </div>

            <div className="mt-2 flex items-center gap-2">
              <button
                className={btnClass}
                style={btnStyle}
                onClick={restart}
                disabled={busy === "restart"}
              >
                {busy === "restart" ? "重启中…" : "重启引擎"}
              </button>
              <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
                档位改动后需重启生效
              </span>
            </div>
          </section>

          {/* 术语表 */}
          <section className="mb-4">
            <div className="mb-1.5 flex items-baseline gap-2">
              <span
                className="text-[11px] font-medium"
                style={{ color: "var(--text-muted)" }}
              >
                术语表
              </span>
              <span className="text-[10px]" style={{ color: "var(--text-muted)" }}>
                每行一条：原文=译文
              </span>
            </div>

            <textarea
              className="h-24 w-full resize-none rounded-lg border p-2 text-[12px] outline-none focus:border-[var(--brand)]"
              style={fieldStyle}
              placeholder={"API=接口\nLLM=大语言模型"}
              value={glossaryText}
              onChange={(e) => {
                setGlossaryText(e.target.value);
                setGlossarySaved(false);
              }}
            />

            <div className="mt-1.5 flex items-center gap-2">
              <button className={btnClass} style={btnStyle} onClick={saveGlossary}>
                保存术语表
              </button>
              {glossarySaved && (
                <span className="text-[11px]" style={{ color: "var(--brand-text)" }}>
                  已保存
                </span>
              )}
            </div>
          </section>

          {/* 模型目录 */}
          <div className="mb-3">
            <div className="mb-1 text-[11px]" style={{ color: "var(--text-muted)" }}>
              模型目录
            </div>
            <div className="flex items-center gap-2">
              <span
                className="min-w-0 flex-1 truncate text-[12px]"
                title={settings?.modelsDir}
              >
                {settings?.modelsDir ?? "—"}
              </span>
              <button
                className={btnClass}
                style={btnStyle}
                onClick={() => openModelsDir()}
              >
                打开
              </button>
              <button className={btnClass} style={btnStyle} onClick={changeDir}>
                切换
              </button>
            </div>
          </div>

          {/* 模型列表（单列） */}
          <div className="mb-1 flex items-center gap-2">
            <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
              共 {local.length} 个模型
            </span>
            <button
              className={`ml-auto ${btnClass}`}
              style={btnStyle}
              onClick={refresh}
            >
              刷新
            </button>
          </div>

          <ul className="m-0 flex list-none flex-col gap-2 p-0">
            {local.length === 0 && (
              <li className="text-[12px]" style={{ color: "var(--text-muted)" }}>
                目录里还没有 .gguf 文件
              </li>
            )}

            {local.map((m) => {
              const state = states[m.id] ?? "unloaded";
              const { text, hot } = stateLabel(state);
              const isPinned = pinned.includes(m.id);
              const working = state === "loading" || state === "unloading";

              return (
                <li
                  key={m.id}
                  className="rounded-lg border p-2.5"
                  style={{
                    background: "var(--surface-muted)",
                    borderColor: isPinned
                      ? "color-mix(in srgb, var(--brand) 55%, var(--border))"
                      : "var(--border)",
                  }}
                >
                  <div className="flex items-center gap-2">
                    <span
                      className="min-w-0 flex-1 truncate text-[13px] font-medium"
                      title={m.id}
                    >
                      {m.id}
                    </span>
                    <span
                      className="shrink-0 rounded-full px-1.5 py-0.5 text-[10px]"
                      style={{
                        background: hot ? "var(--brand-soft)" : "transparent",
                        color: hot ? "var(--brand-text)" : "var(--text-muted)",
                        border: hot ? "none" : "1px solid var(--border)",
                      }}
                    >
                      {text}
                    </span>
                  </div>

                  <div className="mt-1 flex items-center gap-2">
                    <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
                      {m.sizeMb} MB
                    </span>

                    <label className="ml-auto flex cursor-pointer items-center gap-1 text-[11px]">
                      <input
                        type="checkbox"
                        checked={isPinned}
                        onChange={(e) => togglePin(m.id, e.target.checked)}
                        className="accent-[var(--brand)]"
                      />
                      常驻
                    </label>

                    {(state === "loaded" || working) && (
                      <button
                        className={btnClass}
                        style={btnStyle}
                        onClick={() => stopModel(m.id)}
                        disabled={busy === m.id}
                      >
                        {busy === m.id
                          ? "处理中…"
                          : state === "loading"
                            ? "停止加载"
                            : "卸载"}
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>

          {error && (
            <p className="m-0 mt-3 text-[12px]" style={{ color: "var(--danger)" }}>
              {error}
            </p>
          )}
        </div>
      </aside>
    </>
  );
}
