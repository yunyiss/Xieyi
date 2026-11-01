import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { LoadIndicator, toLoadState, type LoadState } from "../components/LoadIndicator";
import { ModelStateBadge, stateLabel } from "../components/ModelStateBadge";
import { RunButton } from "../components/RunButton";
import { Select } from "../components/Select";
import { SOURCE_LANGUAGES, TARGET_LANGUAGES } from "../lib/languages";
import {
  abort,
  getRuntimeStatus,
  listRouterModels,
  newRequestId,
  onModelState,
  onQualityReport,
  onToken,
  onTranslateProgress,
  restartService,
  setModelsDir,
  startService,
  translate,
  type QualityReport,
  type RouterModel,
  type RuntimeStatus,
  type ServiceStatus,
} from "../lib/ipc";
import { useTauriEvent } from "../lib/useTauriEvent";

const LANGUAGES = SOURCE_LANGUAGES;
const TARGETS = TARGET_LANGUAGES;

const selectStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};


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
  /** 打开首启引导（引擎或模型还没配好时，页面只留一个入口）。 */
  onOpenSetup?: () => void;
}

export function Translate({
  refreshSignal = 0,
  onSendToChat,
  onRuntimeChanged,
  onOpenSetup,
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


  /**
   * 把 .gguf 直接拖进窗口：把它所在目录设为模型目录。
   *
   * 不做拷贝——模型动辄几个 GB，复制一份既慢又白占磁盘。
   * 拖进来的若是目录，就用目录本身。
   */
  const adoptDroppedPath = useCallback(async (paths: string[]) => {
    const hit = paths.find((p) => p.toLowerCase().endsWith(".gguf")) ?? paths[0];
    if (!hit) return;
    const dir = /\.gguf$/i.test(hit)
      ? hit.replace(/[\\/][^\\/]+$/, "")
      : hit;
    if (!dir) return;

    setEngineError("");
    try {
      await setModelsDir(dir);
      await restartService();
      await bootstrap();
      onRuntimeChanged?.();
    } catch (e) {
      setEngineError(String(e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 拖放监听：非 Tauri 环境（纯浏览器预览）没有这个 API，兜一层
  useEffect(() => {
    let un: UnlistenFn | null = null;
    let dead = false;
    try {
      getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type === "drop") {
            void adoptDroppedPath(event.payload.paths);
          }
        })
        .then((fn) => {
          // 卸载早于订阅兑现时必须补一次注销，否则监听器会越积越多
          if (dead) fn();
          else un = fn;
        });
    } catch {
      // 忽略：非 Tauri 环境没有拖放能力
    }
    return () => {
      dead = true;
      un?.();
    };
  }, [adoptDroppedPath]);

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

  /** 模型列表非空即算这一步完成（引擎没装时不算，因为拉不到列表）。 */
  const hasModels = models.length > 0;
  const ready = runtime?.installed && hasModels;

  /** 质检检出、但重试后仍未修好的片段——要在译文区明确提示。 */
  const unfixedQuality = quality.filter((q) => !q.fixed);

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
      {/*
        配置引导已移到全屏覆盖层（Onboarding），
        这里只保留一条单行提示 + 重新打开的入口。
        页面本身始终是“配置好之后的样子”。
      */}
      {!ready && (
        <div
          className="flex shrink-0 items-center gap-2 rounded-lg border px-3 py-1.5 text-[12px]"
          style={{
            background: "var(--surface-muted)",
            borderColor: "var(--border)",
            color: "var(--text-muted)",
          }}
        >
          <span className="min-w-0 flex-1 truncate">
            {engineError || "还没配置好引擎或模型"}
          </span>
          {onOpenSetup && (
            <button
              className="btn shrink-0 rounded px-1.5 text-[11px]"
              style={{ color: "var(--brand-text)" }}
              onClick={onOpenSetup}
            >
              去配置
            </button>
          )}
        </div>
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
          options={TARGETS}
          onChange={setTarget}
          title="译文语言"
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
            {/*
              质检没修好时的提示。
              做成“左侧竖线 + 淡字”的行内提示，而不是带红框的警告条：
              红框会把整个译文区变成“错误状态”，但用户此时是有译文的，
              只是质量存疑，不该让人以为整个结果作废。
            */}
            {unfixedQuality.length > 0 && output && !busy && (
              <div
                className="mb-2.5 pl-2.5 text-[11px] leading-relaxed"
                style={{
                  borderLeft: "2px solid var(--border-strong)",
                  color: "var(--text-muted)",
                }}
                title={unfixedQuality
                  .map((r) => `「${r.segment}…」${r.found}`)
                  .join("\n")}
              >
                本次译文未通过质量检查，原因：{unfixedQuality[0].found}。
                可尝试更换参数量更大的模型，或将结果转入对话页进一步调整。
              </div>
            )}
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
