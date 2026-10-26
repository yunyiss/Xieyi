import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { LoadIndicator, toLoadState, type LoadState } from "../components/LoadIndicator";
import { ModelStateBadge } from "../components/ModelStateBadge";
import {
  abort,
  getRuntimeStatus,
  installRuntime,
  listRouterModels,
  newRequestId,
  onModelState,
  onRuntimeProgress,
  onToken,
  onTranslateProgress,
  openModelsDir,
  setManualRuntime,
  setModelsDir,
  startService,
  stopService,
  translate,
  type RouterModel,
  type RuntimeProgress,
  type RuntimeStatus,
  type ServiceStatus,
} from "../lib/ipc";

const LANGUAGES = [
  { code: "auto", label: "自动检测" },
  { code: "zh", label: "中文" },
  { code: "en", label: "英语" },
  { code: "ja", label: "日语" },
  { code: "ko", label: "韩语" },
  { code: "fr", label: "法语" },
  { code: "de", label: "德语" },
  { code: "es", label: "西班牙语" },
  { code: "ru", label: "俄语" },
];

const selectStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

const selectClass =
  "h-9 rounded-lg border px-2.5 text-[13px] outline-none transition-colors focus:border-[var(--brand)]";

function stageLabel(stage: RuntimeProgress["stage"]): string {
  switch (stage) {
    case "downloading":
      return "下载中";
    case "verifying":
      return "校验中";
    case "extracting":
      return "解压中";
    case "checking":
      return "自检中";
    default:
      return "完成";
  }
}

interface Props {
  /** 设置抽屉里改动后递增，用于触发重新扫描。 */
  refreshSignal?: number;
  /** 把译文送到对话页继续调整语意。 */
  onSendToChat?: (text: string) => void;
  /** 运行时安装/切换成功后通知父级，让常驻挂载的其他页面也重新拉取模型。 */
  onRuntimeChanged?: () => void;
}

export function Translate({
  refreshSignal = 0,
  onSendToChat,
  onRuntimeChanged,
}: Props) {
  const [source, setSource] = useState("auto");
  const [target, setTarget] = useState("zh");
  const [input, setInput] = useState("");
  const [output, setOutput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [service, setService] = useState<ServiceStatus | null>(null);
  const [models, setModels] = useState<RouterModel[]>([]);
  const [model, setModel] = useState("");
  const [progress, setProgress] = useState<RuntimeProgress | null>(null);
  const [engineError, setEngineError] = useState("");
  const [load, setLoad] = useState<LoadState | null>(null);
  const [segmentProgress, setSegmentProgress] = useState({
    done: 0,
    total: 0,
  });

  const unlistenRef = useRef<UnlistenFn | null>(null);

  /** 走完整链路：确认运行时 → 拉起引擎 → 拉模型列表 */
  const bootstrap = useCallback(async () => {
    setEngineError("");
    try {
      const rt = await getRuntimeStatus();
      setRuntime(rt);
      if (!rt.installed) return;

      await startService().then(setService);
      const list = await listRouterModels();
      setModels(list);
      setModel((prev) => prev || list[0]?.id || "");
    } catch (e) {
      setEngineError(String(e));
    }
  }, []);

  useEffect(() => {
    bootstrap();
  }, [bootstrap, refreshSignal]);

  useEffect(() => {
    let un: UnlistenFn | null = null;
    onRuntimeProgress(setProgress).then((fn) => (un = fn));
    return () => {
      un?.();
    };
  }, []);

  // 长文分段翻译的进度
  useEffect(() => {
    let un: UnlistenFn | null = null;
    onTranslateProgress(setSegmentProgress).then((fn) => (un = fn));
    return () => {
      un?.();
    };
  }, []);

  // 模型状态实时同步（加载中 / 已加载 / 卸载中）
  useEffect(() => {
    let un: UnlistenFn | null = null;
    onModelState((p) => {
      const mapped =
        p.state === "warm" ? "loaded" : p.state === "cold" ? "unloaded" : p.state;
      setModels((prev) =>
        prev.map((m) => (m.id === p.model ? { ...m, state: mapped } : m)),
      );
      setLoad(toLoadState(p));
    }).then((fn) => (un = fn));
    return () => {
      un?.();
    };
  }, []);

  useEffect(() => {
    return () => {
      unlistenRef.current?.();
    };
  }, []);

  const handleInstall = async (backend: string) => {
    setEngineError("");
    try {
      const rt = await installRuntime(backend);
      setRuntime(rt);
      setProgress(null);
      await bootstrap();
      // 批量页/对话页是常驻挂载的，不会自己重新拉模型；装完运行时统一通知一次
      onRuntimeChanged?.();
    } catch (e) {
      setProgress(null);
      setEngineError(String(e));
    }
  };

  const handleManual = async () => {
    const path = window.prompt(
      "输入已有的 llama-server 可执行文件完整路径：",
      "D:\\llama-b11714-bin-win-cuda-13.4-x64\\llama-server.exe",
    );
    if (!path) return;
    try {
      setRuntime(await setManualRuntime(path));
      await bootstrap();
      onRuntimeChanged?.();
    } catch (e) {
      setEngineError(String(e));
    }
  };

  const handleRun = async () => {
    if (!model) {
      setError("请先选择模型");
      return;
    }
    if (!input.trim()) {
      setError("请输入要翻译的内容");
      return;
    }

    setError("");
    setOutput("");
    setBusy(true);
    setSegmentProgress({ done: 0, total: 0 });

    const id = newRequestId();
    unlistenRef.current?.();
    unlistenRef.current = await onToken((p) => {
      if (p.requestId === id) setOutput((prev) => prev + p.token);
    });

    try {
      const text = await translate({
        requestId: id,
        model,
        source,
        target,
        text: input,
      });
      setOutput(text);
    } catch (e) {
      setError(String(e));
    } finally {
      unlistenRef.current?.();
      unlistenRef.current = null;
      setBusy(false);
    }
  };

  const ready = runtime?.installed && models.length > 0;

  const currentModelState =
    models.find((m) => m.id === model)?.state ?? "unloaded";

  const handleStop = async () => {
    try {
      await abort(model || undefined);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      {/* 运行时 / 引擎状态 */}
      {!runtime?.installed && (
        <section
          className="rounded-xl border p-3"
          style={{ background: "var(--surface)", borderColor: "var(--border)" }}
        >
          <div className="mb-2 flex items-center gap-2 text-[13px] font-medium">
            <span>首次使用：安装推理运行时</span>
            <span
              className="rounded-full px-2 py-0.5 text-[11px]"
              style={{ background: "var(--brand-soft)", color: "var(--brand-text)" }}
            >
              llama.cpp b11714
            </span>
          </div>

          {progress ? (
            <div>
              <div
                className="mb-1 flex justify-between text-[12px]"
                style={{ color: "var(--text-muted)" }}
              >
                <span>{stageLabel(progress.stage)}</span>
                <span>
                  {progress.stage === "downloading"
                    ? `${progress.percent.toFixed(1)}% · ${(
                        progress.downloaded /
                        1048576
                      ).toFixed(1)} / ${(progress.total / 1048576).toFixed(
                        1,
                      )} MB`
                    : ""}
                </span>
              </div>
              <div
                className="h-1.5 overflow-hidden rounded-full"
                style={{ background: "var(--surface-muted)" }}
              >
                <div
                  className="h-full rounded-full transition-[width] duration-200"
                  style={{
                    width: `${progress.percent}%`,
                    background: "var(--brand)",
                  }}
                />
              </div>
            </div>
          ) : (
            <div className="flex items-center gap-2">
              <button
                className="h-9 rounded-lg px-4 text-[13px] font-medium text-white"
                style={{ background: "var(--brand)" }}
                onClick={() => handleInstall("vulkan")}
                title="下载并解压到应用私有目录，约 31 MB"
              >
                下载运行时（Vulkan · 约 31 MB）
              </button>
              <button
                className="h-9 rounded-lg border px-3 text-[13px]"
                style={{ ...selectStyle }}
                onClick={handleManual}
              >
                使用已有 llama-server
              </button>
            </div>
          )}

          {!progress && (
            <p
              className="m-0 mt-2 text-[11px]"
              style={{ color: "var(--text-muted)" }}
            >
              Vulkan 为通用后端，首次推理会有约 3 秒着色器编译；NVIDIA 用户若追求更低延迟，
              可用「使用已有 llama-server」指向本机 CUDA 版。
            </p>
          )}

          {engineError && (
            <p className="m-0 mt-2 text-[12px]" style={{ color: "var(--danger)" }}>
              {engineError}
            </p>
          )}
        </section>
      )}

      {runtime?.installed && models.length === 0 && !engineError && (
        <section
          className="rounded-xl border p-3 text-[13px]"
          style={{ background: "var(--surface)", borderColor: "var(--border)" }}
        >
          <span style={{ color: "var(--text-muted)" }}>
            模型目录为空，把 .gguf 文件放进去后重新扫描。
          </span>
          <button
            className="ml-3 h-8 rounded-lg border px-3 text-[12px]"
            style={selectStyle}
            onClick={async () => {
              await openModelsDir();
              setEngineError("");
              bootstrap();
            }}
          >
            打开模型目录
          </button>

          <button
            className="ml-2 h-8 rounded-lg border px-3 text-[12px]"
            style={selectStyle}
            onClick={async () => {
              const dir = window.prompt(
                "输入已有的模型目录（存放 .gguf 文件）：",
                "D:\\LLM",
              );
              if (!dir) return;
              try {
                await setModelsDir(dir);
                await stopService();
                await bootstrap();
              } catch (e) {
                setEngineError(String(e));
              }
            }}
          >
            使用已有目录
          </button>
        </section>
      )}

      {/* 工具栏：强制单行，窗口变窄时只压缩模型下拉，不换行 */}
      <div className="flex flex-nowrap items-center gap-2">
        <select
          className={`${selectClass} w-[104px] shrink-0`}
          style={selectStyle}
          value={source}
          onChange={(e) => setSource(e.target.value)}
        >
          {LANGUAGES.map((l) => (
            <option key={l.code} value={l.code}>
              {l.label}
            </option>
          ))}
        </select>

        <span className="shrink-0" style={{ color: "var(--text-muted)" }}>
          →
        </span>

        <select
          className={`${selectClass} w-[96px] shrink-0`}
          style={selectStyle}
          value={target}
          onChange={(e) => setTarget(e.target.value)}
        >
          {LANGUAGES.filter((l) => l.code !== "auto").map((l) => (
            <option key={l.code} value={l.code}>
              {l.label}
            </option>
          ))}
        </select>

        <span
          className="mx-0.5 h-5 w-px shrink-0"
          style={{ background: "var(--border)" }}
        />

        <select
          className={`${selectClass} min-w-0 flex-1`}
          style={selectStyle}
          value={model}
          onChange={(e) => setModel(e.target.value)}
          disabled={models.length === 0}
          title={model || "未发现模型"}
        >
          {models.length === 0 && <option value="">未发现模型</option>}
          {models.map((m) => (
            <option key={m.id} value={m.id}>
              {m.id}
            </option>
          ))}
        </select>

        <ModelStateBadge state={currentModelState} />
      </div>

      {load && <LoadIndicator load={load} />}

      {/* 输入 / 输出 */}
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 lg:grid-cols-2">
        <section
          className="flex min-h-0 flex-col overflow-hidden rounded-xl border"
          style={{ background: "var(--surface)", borderColor: "var(--border)" }}
        >
          <header
            className="flex items-center justify-between border-b px-3 py-2 text-[12px]"
            style={{ borderColor: "var(--border)", color: "var(--text-muted)" }}
          >
            <span>原文</span>
            <span>{input.length} 字</span>
          </header>
          <textarea
            className="min-h-0 flex-1 resize-none bg-transparent p-3 outline-none"
            placeholder="粘贴或输入要翻译的内容…"
            value={input}
            onChange={(e) => setInput(e.target.value)}
          />
        </section>

        <section
          className="flex min-h-0 flex-col overflow-hidden rounded-xl border"
          style={{ background: "var(--surface)", borderColor: "var(--border)" }}
        >
          <header
            className="flex items-center justify-between border-b px-3 py-2 text-[12px]"
            style={{ borderColor: "var(--border)", color: "var(--text-muted)" }}
          >
            <span>译文</span>
            <span>
              {busy
                ? segmentProgress.total > 1
                  ? `翻译中 ${segmentProgress.done}/${segmentProgress.total}`
                  : "翻译中…"
                : model || "—"}
            </span>
          </header>
          <div className="min-h-0 flex-1 overflow-auto p-3">
            {engineError && !output ? (
              <p className="m-0 text-[12px]" style={{ color: "var(--danger)" }}>
                {engineError}
              </p>
            ) : output ? (
              <p className="m-0 whitespace-pre-wrap">{output}</p>
            ) : (
              <p className="m-0" style={{ color: "var(--text-muted)" }}>
                译文会显示在这里
              </p>
            )}
          </div>
        </section>
      </div>

      {/* 操作区 */}
      <div className="flex items-center gap-3">
        <button
          className="h-10 rounded-lg px-5 text-[13px] font-medium text-white transition-colors disabled:opacity-50"
          style={{ background: "var(--brand)" }}
          onMouseEnter={(e) =>
            (e.currentTarget.style.background = "var(--brand-hover)")
          }
          onMouseLeave={(e) =>
            (e.currentTarget.style.background = "var(--brand)")
          }
          disabled={!ready || busy}
          onClick={handleRun}
        >
          {load?.state === "warming"
            ? "预热中…"
            : load
              ? "加载模型中…"
              : busy
                ? "翻译中…"
                : "运行翻译"}
        </button>

        {busy && (
          <button
            className="h-10 rounded-lg border px-4 text-[13px]"
            style={selectStyle}
            onClick={handleStop}
          >
            {load ? "停止加载" : "停止"}
          </button>
        )}

        {error && (
          <span className="text-[12px]" style={{ color: "var(--danger)" }}>
            {error}
          </span>
        )}
        {!error && service && (
          <span className="text-[12px]" style={{ color: "var(--text-muted)" }}>
            引擎 127.0.0.1:{service.port}
          </span>
        )}

        {output && onSendToChat && (
          <button
            className="h-10 rounded-lg border px-4 text-[13px]"
            style={selectStyle}
            onClick={() => onSendToChat(output)}
            title="把译文带到对话页，可以润色、改语意、增减内容"
          >
            在对话中调整
          </button>
        )}

        <button
          className="ml-auto h-10 rounded-lg border px-4 text-[13px]"
          style={selectStyle}
          onClick={() => {
            setInput("");
            setOutput("");
            setError("");
          }}
        >
          清空
        </button>
      </div>
    </div>
  );
}
