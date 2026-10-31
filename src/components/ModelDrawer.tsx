import { useCallback, useEffect, useRef, useState } from "react";
// 这个组件的 props 里已经有 `open`（抽屉是否展开），
// 所以文件对话框的函数要改个名字，避免遮蔽。
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  abort,
  discoverRuntimes,
  getGlossary,
  getRuntimeStatus,
  getSettings,
  listLocalModels,
  listRouterModels,
  openModelsDir,
  probeVram,
  restartService,
  setGlossary,
  setGpuLayers,
  setManualRuntime,
  setModelParams,
  setModelChatCapable,
  setModelsDir,
  setPinned,
  setTtl,
  type DiscoveredRuntime,
  type GlossaryEntry,
  type LocalModel,
  type RuntimeProbe,
  type RuntimeStatus,
  type Settings,
} from "../lib/ipc";
import { Select } from "./Select";
import { StorageInfo } from "./StorageInfo";

const TTL_OPTIONS = [
  { value: 60, label: "空闲 60 秒释放" },
  { value: 120, label: "空闲 120 秒释放" },
  { value: 300, label: "空闲 5 分钟释放" },
  { value: 0, label: "不自动释放" },
];

const LAYER_OPTIONS = [
  { value: 1001, label: "自动（按显存）" },
  { value: 999, label: "全 GPU" },
  { value: 16, label: "混合 16 层" },
  { value: 0, label: "纯 CPU" },
];

/** 可选的上下文档位：不超过模型原生上限，且保留当前值。 */
function ctxOptions(model: LocalModel): number[] {
  const max = model.trainedCtx ?? 32768;
  const list = [2048, 4096, 8192, 16384, 32768, 65536].filter((v) => v <= max);
  if (!list.includes(model.ctxSize)) list.push(model.ctxSize);
  return list.sort((a, b) => a - b);
}

interface Props {
  open: boolean;
  onClose: () => void;
  onChanged: () => void;
  /** 重新打开首启引导（引擎或模型换地方时，比逐项改更快）。 */
  onOpenGuide?: () => void;
}

const btnClass =
  "btn h-7 rounded-md border px-2 text-[11px] transition-colors disabled:opacity-50";

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
export function ModelDrawer({ open, onClose, onChanged, onOpenGuide }: Props) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [local, setLocal] = useState<LocalModel[]>([]);
  /** 当前页签：engines 类参数与模型分开，避免一页塞太多。 */
  const [tab, setTab] = useState<"general" | "models">("general");
  /** 当前推理引擎状态，用于「推理引擎」那一块。 */
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  /** 引擎切换后的自检结果（成功显示版本，失败显示原因）。 */
  const [probeMsg, setProbeMsg] = useState<RuntimeProbe | null>(null);
  /** 扫描到的可切换引擎。 */
  const [discovered, setDiscovered] = useState<DiscoveredRuntime[]>([]);
  const [states, setStates] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [glossaryText, setGlossaryText] = useState("");
  const [glossarySaved, setGlossarySaved] = useState(false);
  const [freeVram, setFreeVram] = useState<number | null>(null);
  /** 改过上下文后需要重启引擎才生效 */
  const [needsRestart, setNeedsRestart] = useState(false);
  /** 退场动画期间仍然渲染，动画结束后再卸载 */
  const [closing, setClosing] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const s = await getSettings();
      setSettings(s);
      setLocal(await listLocalModels());
      setFreeVram(await probeVram());
      setRuntime(await getRuntimeStatus());

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

  // 关闭时先播 170ms 退场动画再卸载，避免"啪一下消失"。
  // 用 wasOpen 记住"真的打开过"，否则首次挂载会误播一次退场。
  const wasOpen = useRef(false);
  useEffect(() => {
    if (open) {
      wasOpen.current = true;
      setClosing(false);
      return;
    }
    if (!wasOpen.current) return;
    wasOpen.current = false;
    setClosing(true);
    const timer = window.setTimeout(() => setClosing(false), 170);
    return () => window.clearTimeout(timer);
  }, [open]);

  // 按 Esc 关闭
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open && !closing) return null;

  const pinned = settings?.pinned ?? [];

  /**
   * 切换推理引擎。
   *
   * 后端会先跑 `--version` 自检，跑不起来的文件直接拒绝——
   * 否则会被记成"已安装"，直到真正翻译时才炸。
   * 成功后要把引擎重启，否则旧进程还在用旧的可执行文件。
   */
  const useEngine = async (path: string) => {
    setError("");
    setProbeMsg(null);
    setBusy("engine");
    try {
      const st = await setManualRuntime(path);
      setRuntime(st);
      setProbeMsg(st.probe ?? null);
      // 换引擎必须重启，否则推理还在跑旧的那个
      await restartService();
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  /** 用系统文件对话框挑一个 llama-server 可执行文件。 */
  const pickEngineFile = async () => {
    setError("");
    setProbeMsg(null);
    try {
      const picked = await openDialog({
        multiple: false,
        directory: false,
        title: "选择 llama-server 可执行文件",
        filters: [
          {
            name: "可执行文件",
            extensions: ["exe"],
          },
        ],
      });
      if (typeof picked !== "string") return;
      await useEngine(picked);
    } catch (e) {
      setError(String(e));
    }
  };

  /** 扫一遍磁盘，列出可以一键切换的引擎。 */
  const scanEngines = async () => {
    setError("");
    setBusy("engine");
    try {
      setDiscovered(await discoverRuntimes());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

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

  /**
   * 切换"能不能用在对话页"。
   *
   * 勾回去"自动"是没有单独按钮的——用户再点一次就得到与自动相反的结果，
   * 那反而是"手动"。所以这里只在值等于自动判定时清掉覆盖，
   * 让"手动改回原值"自然地恢复成自动。
   */
  const toggleChat = async (id: string, next: boolean) => {
    setError("");
    try {
      const m = local.find((x) => x.id === id);
      // 改回与自动一致的值，等于恢复自动识别，避免留下无意义的覆盖记录
      await setModelChatCapable(id, m && next === m.chatAuto ? null : next);
      await refresh();
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

  /** 一键让位：卸载当前所有驻留模型，把显存还给前台程序。 */
  const releaseAll = async () => {
    setError("");
    setBusy("release");
    try {
      await abort();
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
      setNeedsRestart(false);
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  /** 改单个模型的上下文；引擎在启动时读 preset，所以必须重启才生效。 */
  const changeModelCtx = async (m: LocalModel, ctx: number) => {
    setError("");
    try {
      const saved = await setModelParams(m.id, ctx, m.thinking);
      setLocal((prev) =>
        prev.map((x) => (x.id === m.id ? { ...x, ctxSize: saved.ctx } : x)),
      );
      setNeedsRestart(true);
    } catch (e) {
      setError(String(e));
    }
  };

  /**
   * 换模型目录。
   *
   * 原来用 `window.prompt` 手打路径：Tauri 只 polyfill 了 alert/confirm，
   * prompt 是 WebView2 原生实现，既不能浏览目录也没法校验，打字极易出错。
   */
  const changeDir = async () => {
    setError("");
    try {
      const picked = await openDialog({
        multiple: false,
        directory: true,
        title: "选择存放 .gguf 的模型目录",
        defaultPath: settings?.modelsDir || undefined,
      });
      if (typeof picked !== "string") return;
      setBusy("dir");
      await setModelsDir(picked);
      await restartService();
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
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
        className="backdrop fixed inset-0 z-40"
        style={{
          background: "color-mix(in srgb, var(--text) 18%, transparent)",
          backdropFilter: "blur(2px)",
        }}
        onClick={onClose}
      />
      <aside
        className="drawer fixed bottom-2 right-2 top-2 z-50 flex w-[380px] flex-col overflow-hidden rounded-[16px]"
        style={{
          background: "var(--surface)",
          border: "1px solid var(--border)",
          boxShadow: "var(--shadow-3)",
          maxWidth: "calc(92vw - 16px)",
        }}
        data-closing={closing && !open}
      >
        <header
          className="flex shrink-0 items-center gap-2 border-b px-4 py-3"
          style={{ borderColor: "var(--border)" }}
        >
          <h2 className="m-0 text-[14px] font-medium">设置</h2>
          <button
            className="btn ml-auto flex h-7 w-7 items-center justify-center rounded-lg border"
            style={btnStyle}
            onClick={onClose}
            aria-label="关闭"
            title="关闭（Esc）"
          >
            ✕
          </button>
        </header>

        {/* 两个页签：引擎这类进程级参数放「通用」，模型与术语放「模型」。
            全都堆一页时已经要滚半天，再加能力开关就彻底找不着了。 */}
        <div
          className="flex shrink-0 items-center gap-1 border-b px-3 py-1.5"
          style={{ borderColor: "var(--border)" }}
          role="tablist"
        >
          {(
            [
              { key: "general", label: "通用" },
              { key: "models", label: "模型与术语" },
            ] as const
          ).map((t) => {
            const active = tab === t.key;
            return (
              <button
                key={t.key}
                role="tab"
                aria-selected={active}
                className="btn rounded-lg px-2.5 py-1 text-[12px]"
                style={{
                  background: active ? "var(--brand-soft)" : "transparent",
                  color: active ? "var(--brand-text)" : "var(--text-muted)",
                  fontWeight: active ? 500 : 400,
                }}
                onClick={() => setTab(t.key)}
              >
                {t.label}
              </button>
            );
          })}
          <span
            className="ml-auto text-[11px]"
            style={{ color: "var(--text-muted)" }}
          >
            {local.length} 个模型
          </span>
        </div>

        <div className="min-h-0 flex-1 overflow-auto px-4 py-3">
          {/* 引擎设置 */}
          {tab === "general" && (
          <section className="mb-4">
            <div className="section-title mb-1.5">引擎设置</div>

            <div className="flex flex-col gap-2">
              <div className="flex items-center gap-2 text-[12px]">
                <span className="w-[64px] shrink-0" style={{ color: "var(--text-muted)" }}>
                  空闲释放
                </span>
                <Select
                  className="min-w-0 flex-1"
                  size="sm"
                  value={settings?.ttlSecs ?? 120}
                  options={TTL_OPTIONS}
                  onChange={(v) => changeTtl(v)}
                  title="模型空闲多久后自动卸载，释放显存"
                />
              </div>

              <div className="flex items-center gap-2 text-[12px]">
                <span className="w-[64px] shrink-0" style={{ color: "var(--text-muted)" }}>
                  推理档位
                </span>
                <Select
                  className="min-w-0 flex-1"
                  size="sm"
                  value={settings?.gpuLayers ?? 1001}
                  options={LAYER_OPTIONS}
                  onChange={(v) => changeLayers(v)}
                  title="给 GPU 分配多少层，自动档按空闲显存估算"
                />
              </div>
            </div>

            <p className="m-0 mt-1 text-[11px]" style={{ color: "var(--text-muted)" }}>
              {freeVram != null
                ? `当前可用显存 ${freeVram} MB`
                : "未检测到可用显存（非 NVIDIA 或无驱动）"}
              {settings?.gpuLayers === 1001 ? " · 自动档按模型体积分配层数" : ""}
            </p>

            <div className="mt-2 flex items-center gap-2">
              <button
                className={btnClass}
                style={btnStyle}
                onClick={restart}
                disabled={busy === "restart"}
              >
                {busy === "restart" ? "重启中…" : "重启引擎"}
              </button>
              <button
                className={btnClass}
                style={btnStyle}
                onClick={releaseAll}
                disabled={busy === "release"}
              >
                {busy === "release" ? "处理中…" : "一键让位"}
              </button>
              <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
                {needsRestart ? "上下文已改，重启后生效" : "档位改动后需重启生效"}
              </span>
            </div>
          </section>
          )}

          {tab === "general" && (
          <section className="mb-4">
            <div className="flex items-baseline gap-2">
              <span className="section-title">推理引擎</span>
              <span className="text-[10px]" style={{ color: "var(--text-muted)" }}>
                换成自己的 llama-server，不必重新下载
              </span>
            </div>

            <div
              className="mt-1.5 rounded-[10px] border p-2.5"
              style={{
                background: "var(--surface-muted)",
                borderColor: "var(--border)",
              }}
            >
              <p
                className="m-0 truncate text-[12px]"
                title={runtime?.serverPath ?? ""}
                style={{ color: "var(--text)" }}
              >
                {runtime?.installed
                  ? (runtime.serverPath ?? "").split(/[\\/]/).slice(-2).join(" / ")
                  : "尚未安装推理引擎"}
              </p>
              <p className="m-0 mt-0.5 text-[11px]" style={{ color: "var(--text-muted)" }}>
                {runtime?.installed
                  ? `${runtime.backend ?? "未知"} · ${runtime.version ?? "未知版本"}`
                  : "还没配置过，可以走一遍引导"}
              </p>

              {onOpenGuide && (
                <button
                  className="btn mt-2 h-7 rounded-md border px-2 text-[11px]"
                  style={btnStyle}
                  onClick={() => {
                    onClose();
                    onOpenGuide();
                  }}
                >
                  重新打开配置引导
                </button>
              )}

              {probeMsg && (
                <p
                  className="m-0 mt-1.5 text-[11px] leading-relaxed"
                  style={{
                    color: probeMsg.ok ? "var(--brand-text)" : "var(--danger)",
                  }}
                >
                  {probeMsg.ok ? "✓ " : "✗ "}
                  {probeMsg.detail}
                </p>
              )}

              <div className="mt-2 flex flex-wrap items-center gap-1.5">
                <button
                  className={btnClass}
                  style={btnStyle}
                  onClick={pickEngineFile}
                  disabled={busy === "engine"}
                >
                  选择文件…
                </button>
                <button
                  className={btnClass}
                  style={btnStyle}
                  onClick={scanEngines}
                  disabled={busy === "engine"}
                >
                  {busy === "engine" ? "扫描中…" : "扫描已安装"}
                </button>
              </div>

              {/* 扫出来的候选：点一下就能换，省得去文件对话框翻路径 */}
              {discovered.length > 0 && (
                <ul className="m-0 mt-2 flex list-none flex-col gap-1 p-0">
                  {discovered.map((d) => {
                    const current = d.path === runtime?.serverPath;
                    return (
                      <li key={d.path}>
                        <button
                          className="row w-full rounded-lg px-2 py-1.5 text-left"
                          data-selected={current}
                          disabled={busy === "engine" || current}
                          onClick={() => useEngine(d.path)}
                          title={d.path}
                        >
                          <span className="flex items-center gap-1.5">
                            <span className="min-w-0 flex-1 truncate text-[12px]">
                              {d.label}
                            </span>
                            <span
                              className="shrink-0 rounded px-1 py-0.5 text-[10px]"
                              style={{
                                background: "var(--brand-soft)",
                                color: "var(--brand-text)",
                              }}
                            >
                              {d.backend}
                            </span>
                            {current && (
                              <span
                                className="shrink-0 text-[10px]"
                                style={{ color: "var(--brand-text)" }}
                              >
                                当前
                              </span>
                            )}
                          </span>
                          <span
                            className="mt-0.5 block truncate text-[10px]"
                            style={{ color: "var(--text-muted)" }}
                          >
                            {d.version}
                          </span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>
          </section>
          )}

          {tab === "general" && <StorageInfo />}

          {tab === "models" && (
          <>
          {/* 术语表 */}
          <section className="mb-4">
            <div className="mb-1.5 flex items-baseline gap-2">
              <span className="section-title">术语表</span>
              <span className="text-[10px]" style={{ color: "var(--text-muted)" }}>
                每行一条：原文=译文
              </span>
            </div>

            <textarea
              className="field h-24 w-full resize-none p-2 text-[12px]"
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
            <div className="section-title mb-1">模型目录</div>
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
                  className="rounded-[10px] border p-2.5 transition-colors"
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

                  {/* 家族与能力：用户放一堆模型时，这里能一眼看出谁能干什么。
                      不可用模型（如 Seed-X）直接说明原因，别让他自己试出来。 */}
                  <div className="mt-1 flex flex-wrap items-center gap-1.5">
                    {m.family && m.family !== "通用" && (
                      <span
                        className="rounded px-1.5 py-0.5 text-[10px]"
                        style={{
                          background:
                            "color-mix(in srgb, var(--brand) 12%, transparent)",
                          color: "var(--brand-text)",
                        }}
                        title={`识别为 ${m.family} 家族，翻译时会用该家族的推荐调用方式`}
                      >
                        {m.family}
                      </span>
                    )}
                    {!m.unsupportedReason && (
                      <span
                        className="rounded px-1.5 py-0.5 text-[10px]"
                        style={{
                          border: "1px solid var(--border)",
                          color: "var(--text-muted)",
                        }}
                        title={
                          m.chatCapable
                            ? "翻译页、批量页、对话页都可以用"
                            : "只用于翻译与批量；对话页不显示（它会把提问本身翻译掉）"
                        }
                      >
                        {m.chatCapable ? "翻译 + 对话" : "仅翻译"}
                      </span>
                    )}
                  </div>

                  {m.unsupportedReason && (
                    <p
                      className="m-0 mt-1 text-[11px] leading-relaxed"
                      style={{ color: "var(--danger)" }}
                    >
                      {m.unsupportedReason}
                    </p>
                  )}

                  <div className="mt-1 flex items-center gap-2">
                    <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
                      {m.sizeMb} MB
                    </span>

                    {/* 能力开关：勾上就能进对话页。自动识别不准时用它改，
                        不必去动模型文件名——这是识别的兜底。 */}
                    <label
                      className="ml-auto flex cursor-pointer items-center gap-1 text-[11px]"
                      title={
                        m.chatOverridden
                          ? `已手动设置（自动判定为${m.chatAuto ? "可用" : "不可用"}），点击恢复自动`
                          : `自动判定为${m.chatAuto ? "可用" : "不可用"}；手动改过之后以你为准`
                      }
                    >
                      <input
                        type="checkbox"
                        checked={m.chatCapable}
                        onChange={(e) => toggleChat(m.id, e.target.checked)}
                        className="accent-[var(--brand)]"
                      />
                      对话
                      {m.chatOverridden && (
                        <span style={{ color: "var(--brand-text)" }}>·手动</span>
                      )}
                    </label>

                    <label className="flex cursor-pointer items-center gap-1 text-[11px]">
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

                  {/* 单个模型的上下文：进程级参数，只能在这里改；
                      思考强度在对话页输入区选，不在这里重复一份 */}
                  <div className="mt-1.5 flex items-center gap-1.5 text-[11px]">
                    <span className="shrink-0" style={{ color: "var(--text-muted)" }}>
                      上下文
                    </span>
                    <Select
                      className="min-w-0 flex-1"
                      size="sm"
                      value={m.ctxSize}
                      options={ctxOptions(m).map((v) => ({ value: v, label: String(v) }))}
                      onChange={(v) => changeModelCtx(m, v)}
                      title={
                        m.trainedCtx
                          ? `模型原生上限 ${m.trainedCtx}`
                          : "未能从 GGUF 读出原生上限"
                      }
                    />
                  </div>

                  {!m.supportsThinking && (
                    <p
                      className="m-0 mt-1 text-[10px]"
                      style={{ color: "var(--text-muted)" }}
                    >
                      该模型不支持思考，对话页不会显示思考强度
                    </p>
                  )}

                  {m.trainedCtx != null && (
                    <p
                      className="m-0 mt-1 text-[10px]"
                      style={{ color: "var(--text-muted)" }}
                    >
                      原生上下文 {m.trainedCtx}
                    </p>
                  )}
                </li>
              );
            })}
          </ul>
          </>
          )}

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
