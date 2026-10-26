import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { LoadIndicator, toLoadState, type LoadState } from "../components/LoadIndicator";
import { ModelStateBadge, stateLabel } from "../components/ModelStateBadge";
import { RunButton } from "../components/RunButton";
import { Select } from "../components/Select";
import { TRANSLATE_STYLES } from "../lib/translateStyles";
import {
  abort,
  getRuntimeStatus,
  installRuntime,
  listRouterModels,
  newRequestId,
  onModelState,
  onQualityReport,
  onRuntimeProgress,
  onToken,
  onTranslateProgress,
  openModelsDir,
  setManualRuntime,
  setModelsDir,
  startService,
  stopService,
  translate,
  type QualityReport,
  type RouterModel,
  type RuntimeProgress,
  type RuntimeStatus,
  type ServiceStatus,
  type TranslateStyle,
} from "../lib/ipc";
import { useTauriEvent } from "../lib/useTauriEvent";

const LANGUAGES = [
  { value: "auto", label: "自动检测" },
  { value: "zh", label: "中文" },
  { value: "en", label: "英语" },
  { value: "ja", label: "日语" },
  { value: "ko", label: "韩语" },
  { value: "fr", label: "法语" },
  { value: "de", label: "德语" },
  { value: "es", label: "西班牙语" },
  { value: "ru", label: "俄语" },
];

const selectStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

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

/** 质检提示：平时只是一个数字，鼠标悬停才展开细节，不打扰正常翻译。 */
function QualityNote({
  reports,
  total,
}: {
  reports: QualityReport[];
  /** 本次翻译的总片段数，用来显示「2/12 段」而不是孤零零一个数字 */
  total: number;
}) {
  const flagged = reports.length;
  const unfixed = reports.filter((r) => !r.fixed).length;
  const scope = total > 0 ? `/${total}` : "";
  const text =
    unfixed > 0
      ? `质检 ${flagged}${scope} 段 · ${unfixed} 段仍可疑`
      : `质检 ${flagged}${scope} 段 · 均已修正`;

  const detail = reports
    .map(
      (r) =>
        `「${r.segment}…」${r.found}${r.fixed ? "（已修正）" : "（重试后仍可疑）"}`,
    )
    .join("\n");

  return (
    <span
      className="min-w-0 truncate"
      style={{ color: unfixed > 0 ? "var(--danger)" : "var(--brand-text)" }}
      title={detail}
    >
      {text}
    </span>
  );
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
  const [style, setStyle] = useState<TranslateStyle>("general");
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
  /** 本次翻译里被质检挑出问题、已自动重试的片段 */
  const [quality, setQuality] = useState<QualityReport[]>([]);

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

  useTauriEvent(onRuntimeProgress, setProgress);

  // 长文分段翻译的进度
  useTauriEvent(onTranslateProgress, setSegmentProgress);

  // 质检报告：后端发现译文有问题并自动重试过。批量任务的 requestId 为空，不在这里显示
  useTauriEvent(onQualityReport, (p) => {
    if (!p.requestId) return;
    setQuality((prev) => [...prev, p]);
  });

  // 模型状态实时同步（加载中 / 已加载 / 卸载中）
  useTauriEvent(onModelState, (p) => {
    const mapped =
      p.state === "warm" ? "loaded" : p.state === "cold" ? "unloaded" : p.state;
    setModels((prev) =>
      prev.map((m) => (m.id === p.model ? { ...m, state: mapped } : m)),
    );
    setLoad(toLoadState(p));
  });

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
    setQuality([]);

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
        style,
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

  const copyOutput = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      setError("复制失败");
    }
  };

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      {/* 运行时 / 引擎状态 */}
      {!runtime?.installed && (
        <section className="card p-3.5">
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
                className="btn h-9 rounded-lg px-4 text-[13px] font-medium text-white"
                style={{ background: "var(--brand)" }}
                onClick={() => handleInstall("vulkan")}
                title="下载并解压到应用私有目录，约 31 MB"
              >
                下载运行时（Vulkan · 约 31 MB）
              </button>
              <button
                className="btn h-9 rounded-lg border px-3 text-[13px]"
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
        <section className="card p-3.5 text-[13px]">
          <span style={{ color: "var(--text-muted)" }}>
            模型目录为空，把 .gguf 文件放进去后重新扫描。
          </span>
          <button
            className="btn ml-3 h-8 rounded-lg border px-3 text-[12px]"
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
            className="btn ml-2 h-8 rounded-lg border px-3 text-[12px]"
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
        <Select
          className="w-[112px] shrink-0"
          value={source}
          options={LANGUAGES}
          onChange={setSource}
          title="原文语言"
        />

        <span className="shrink-0 text-[12px]" style={{ color: "var(--text-muted)" }}>
          →
        </span>

        <Select
          className="w-[104px] shrink-0"
          value={target}
          options={LANGUAGES.filter((l) => l.value !== "auto")}
          onChange={setTarget}
          title="译文语言"
        />

        <Select
          className="w-[92px] shrink-0"
          value={style}
          options={TRANSLATE_STYLES}
          onChange={setStyle}
          title="翻译风格"
        />

        <span
          className="mx-0.5 h-5 w-px shrink-0"
          style={{ background: "var(--border)" }}
        />

        <Select
          className="min-w-0 flex-1"
          value={model}
          options={models.map((m) => ({
            value: m.id,
            label: m.id,
            hint: stateLabel(m.state),
          }))}
          onChange={setModel}
          disabled={models.length === 0}
          placeholder="未发现模型"
          title={model || "未发现模型"}
        />

        <ModelStateBadge state={currentModelState} />
      </div>

      {load && <LoadIndicator load={load} />}

      {/* 输入 / 输出 */}
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 lg:grid-cols-2">
        <section className="card flex min-h-0 flex-col overflow-hidden">
          <header className="panel-head">
            <span>原文</span>
            <span className="tabular-nums opacity-80">{input.length} 字</span>
          </header>
          <textarea
            className="min-h-0 flex-1 resize-none bg-transparent px-3.5 py-3 text-[13px] leading-relaxed outline-none placeholder:opacity-40"
            placeholder="粘贴或输入要翻译的内容…"
            value={input}
            onChange={(e) => setInput(e.target.value)}
          />
        </section>

        <section className="card flex min-h-0 flex-col overflow-hidden">
          <header className="panel-head">
            <span>译文</span>
            <span className="flex min-w-0 items-center gap-2.5">
              {quality.length > 0 && (
                <QualityNote reports={quality} total={segmentProgress.total} />
              )}
              {output && !busy && (
                <button
                  className="btn rounded px-1 text-[11px] opacity-70 hover:opacity-100"
                  onClick={() => copyOutput(output)}
                  title="复制译文"
                >
                  复制
                </button>
              )}
              <span className="shrink-0 tabular-nums">
                {busy
                  ? segmentProgress.total > 1
                    ? `翻译中 ${segmentProgress.done}/${segmentProgress.total}`
                    : "翻译中…"
                  : model || "—"}
              </span>
            </span>
          </header>
          <div className="min-h-0 flex-1 overflow-auto px-3.5 py-3">
            {engineError && !output ? (
              <p className="m-0 text-[12px]" style={{ color: "var(--danger)" }}>
                {engineError}
              </p>
            ) : output ? (
              <p className="m-0 whitespace-pre-wrap text-[13px] leading-relaxed">
                {output}
              </p>
            ) : (
              <p className="m-0 text-[13px]" style={{ color: "var(--text-muted)" }}>
                译文会显示在这里
              </p>
            )}
          </div>
        </section>
      </div>

      {/* 操作区 */}
      <div className="flex items-center gap-3">
        {/* 开始 / 停止共用一个键，与批量页共用同一个组件，保证完全一致 */}
        <RunButton
          busy={busy}
          load={load}
          label="开始翻译"
          disabled={!busy && !ready}
          onClick={busy ? handleStop : handleRun}
        />

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
            className="btn h-10 rounded-lg border px-4 text-[13px]"
            style={selectStyle}
            onClick={() => onSendToChat(output)}
            title="把译文带到对话页，可以润色、改语意、增减内容"
          >
            在对话中调整
          </button>
        )}

        <button
          className="btn ml-auto h-10 rounded-lg border px-4 text-[13px]"
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
