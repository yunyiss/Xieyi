import { useCallback, useEffect, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { LoadIndicator, toLoadState, type LoadState } from "../components/LoadIndicator";
import { ModelStateBadge, stateLabel } from "../components/ModelStateBadge";
import { RunButton } from "../components/RunButton";
import { Select } from "../components/Select";
import { TRANSLATE_STYLES } from "../lib/translateStyles";
import { useTauriEvent } from "../lib/useTauriEvent";
import {
  baseName,
  batchCancel,
  batchTranslate,
  filterBatchPaths,
  getRuntimeStatus,
  listRouterModels,
  onBatchProgress,
  onModelState,
  pickBatchFiles,
  startService,
  type BatchStage,
  type BatchSummary,
  type RouterModel,
  type RuntimeStatus,
  type TranslateStyle,
} from "../lib/ipc";

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

const btnStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

/** 队列项状态：后端阶段之外多一个「排队」。 */
type Stage = BatchStage | "queued";

interface QueueItem {
  path: string;
  name: string;
  stage: Stage;
  done: number;
  segments: number;
  failedSegments: number;
  output?: string;
  error?: string;
}

const STAGE_LABEL: Record<Stage, string> = {
  queued: "排队",
  reading: "读取中",
  translating: "翻译中",
  writing: "写出中",
  done: "完成",
  failed: "失败",
  skipped: "跳过",
};

/** 只有这三个阶段算「正在跑」，用来点亮高亮色。 */
function isActive(stage: Stage): boolean {
  return stage === "reading" || stage === "translating" || stage === "writing";
}

function stageColor(stage: Stage): string {
  if (stage === "failed") return "var(--danger)";
  if (stage === "done") return "var(--brand-text)";
  return "var(--text-muted)";
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  const secs = ms / 1000;
  if (secs < 60) return `${secs.toFixed(1)} 秒`;
  const mins = Math.floor(secs / 60);
  return `${mins} 分 ${Math.round(secs - mins * 60)} 秒`;
}

interface Props {
  /** 设置抽屉里改动后递增，用于触发重新扫描。 */
  refreshSignal?: number;
  /** 当前页签是否可见。隐藏时不接管拖放，避免在别的页面拖文件被静默收进队列。 */
  active?: boolean;
}

/** 批量页：拖入 txt / md / srt，逐个翻译并写到原文件旁边。 */
export function Batch({ refreshSignal = 0, active = true }: Props) {
  const [source, setSource] = useState("auto");
  const [target, setTarget] = useState("zh");
  const [style, setStyle] = useState<TranslateStyle>("general");
  const [models, setModels] = useState<RouterModel[]>([]);
  const [model, setModel] = useState("");
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [queue, setQueue] = useState<QueueItem[]>([]);
  const [running, setRunning] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [error, setError] = useState("");
  const [summary, setSummary] = useState<BatchSummary | null>(null);
  const [load, setLoad] = useState<LoadState | null>(null);

  const bootstrap = useCallback(async () => {
    setError("");
    try {
      const rt = await getRuntimeStatus();
      setRuntime(rt);
      if (!rt.installed) return;

      await startService();
      const list = await listRouterModels();
      setModels(list);
      setModel((prev) => prev || list[0]?.id || "");
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    bootstrap();
  }, [bootstrap, refreshSignal]);

  // 模型状态：更新下拉里的状态标签，同时驱动加载进度条
  useTauriEvent(onModelState, (p) => {
    const mapped =
      p.state === "warm" ? "loaded" : p.state === "cold" ? "unloaded" : p.state;
    setModels((prev) =>
      prev.map((m) => (m.id === p.model ? { ...m, state: mapped } : m)),
    );
    setLoad(toLoadState(p));
  });

  // 批量进度
  useTauriEvent(onBatchProgress, (p) => {
    setQueue((prev) =>
      prev.map((item) =>
        item.path === p.path
          ? {
              ...item,
              stage: p.stage,
              done: p.done,
              segments: p.segments,
              failedSegments: p.failedSegments,
              output: p.output ?? item.output,
              error: p.error,
            }
          : item,
      ),
    );
  });

  /** 把路径加进队列，过滤掉不支持的扩展名。 */
  const addPaths = useCallback((paths: string[]) => {
    const accepted = filterBatchPaths(paths);
    if (accepted.length === 0) {
      setError("只支持 .txt / .md / .srt 文件");
      return;
    }
    setError("");
    setQueue((prev) => {
      const known = new Set(prev.map((i) => i.path));
      const added = accepted
        .filter((p) => !known.has(p))
        .map<QueueItem>((p) => ({
          path: p,
          name: baseName(p),
          stage: "queued",
          done: 0,
          segments: 0,
          failedSegments: 0,
        }));
      return [...prev, ...added];
    });
  }, []);

  // 原生拖放：Tauri 直接给出真实文件路径，比 HTML5 DnD 更可靠
  useEffect(() => {
    if (!active) {
      setDragging(false);
      return;
    }

    let un: UnlistenFn | null = null;
    let dead = false;

    // 拖放走 Tauri 原生 API。在纯浏览器里跑（比如本地预览 UI）它不存在，
    // 抛错会把整页打白，所以这里兜一层：没有原生拖放就退回只有「添加文件」。
    try {
      getCurrentWebview()
        .onDragDropEvent((event) => {
          const payload = event.payload;
          if (payload.type === "over") {
            setDragging(true);
          } else if (payload.type === "leave") {
            setDragging(false);
          } else if (payload.type === "drop") {
            setDragging(false);
            addPaths(payload.paths);
          }
        })
        .then((fn) => {
          // 切换页签常常发生在订阅兑现之前，这里必须补一次注销，
          // 否则每切一次页就多一个拖放监听器（重复触发 drop 会重复入队）
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
  }, [addPaths, active]);

  const pick = async () => {
    try {
      const picked = await pickBatchFiles();
      if (picked) addPaths(picked);
    } catch (e) {
      setError(String(e));
    }
  };

  const start = async () => {
    if (!model || queue.length === 0) return;

    setError("");
    setSummary(null);
    setRunning(true);
    // 重置每一项，避免上一轮的进度残留
    setQueue((prev) =>
      prev.map((item) => ({
        ...item,
        stage: "queued",
        done: 0,
        segments: 0,
        failedSegments: 0,
        output: undefined,
        error: undefined,
      })),
    );

    try {
      const result = await batchTranslate({
        model,
        source,
        target,
        style,
        paths: queue.map((i) => i.path),
      });
      setSummary(result);
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  };

  const stop = async () => {
    try {
      await batchCancel();
    } catch (e) {
      setError(String(e));
    }
  };

  const ready = runtime?.installed && models.length > 0;
  const currentModelState = models.find((m) => m.id === model)?.state ?? "unloaded";
  const finished = queue.filter((i) => i.stage === "done").length;
  const failed = queue.filter((i) => i.stage === "failed").length;

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      {/* 工具栏：与翻译页同构，强制单行 */}
      <div className="flex flex-nowrap items-center gap-2">
        <Select
          className="w-[112px] shrink-0"
          value={source}
          options={LANGUAGES}
          onChange={setSource}
          disabled={running}
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
          disabled={running}
          title="译文语言"
        />

        <Select
          className="w-[92px] shrink-0"
          value={style}
          options={TRANSLATE_STYLES}
          onChange={setStyle}
          disabled={running}
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
          disabled={models.length === 0 || running}
          placeholder="未发现模型"
          title={model || "未发现模型"}
        />

        <ModelStateBadge state={currentModelState} />
      </div>

      {load && <LoadIndicator load={load} />}

      {/* 队列 */}
      <section
        className="card flex min-h-0 flex-1 flex-col overflow-hidden"
        style={{
          background: dragging ? "var(--brand-soft)" : "var(--surface)",
          borderColor: dragging ? "var(--brand)" : "var(--border)",
          boxShadow: dragging ? "var(--shadow-2)" : "var(--shadow-1)",
          transition: "background var(--dur) var(--ease-out), border-color var(--dur) var(--ease-out)",
        }}
      >
        <header className="panel-head">
          <span>
            队列 {queue.length} 个文件
            {queue.length > 0 && ` · 已完成 ${finished} · 失败 ${failed}`}
          </span>
          <span className={dragging ? "" : "opacity-80"}>
            {dragging
              ? "松手即可加入队列"
              : "把 txt / md / srt 拖进来，或点「添加文件」"}
          </span>
        </header>

        <div className="min-h-0 flex-1 overflow-auto p-2">
          {queue.length === 0 ? (
            // 空队列给一个"可以往里拖"的视觉暗示，而不是干巴巴一行字
            <div className="flex h-full items-center justify-center p-4">
              <button
                className="btn flex w-full max-w-[420px] flex-col items-center gap-2 rounded-[14px] border border-dashed px-6 py-10 transition-colors"
                style={{
                  borderColor: dragging ? "var(--brand)" : "var(--border-strong)",
                  background: dragging ? "var(--brand-soft)" : "transparent",
                  color: "var(--text-muted)",
                }}
                onClick={pick}
                disabled={running}
              >
                <svg width="28" height="28" viewBox="0 0 24 24" fill="none" aria-hidden>
                  <path
                    d="M12 16V6m0 0-3.5 3.5M12 6l3.5 3.5M5 16.5v1A2.5 2.5 0 0 0 7.5 20h9a2.5 2.5 0 0 0 2.5-2.5v-1"
                    stroke="currentColor"
                    strokeWidth="1.6"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  />
                </svg>
                <span className="text-[13px]">
                  {dragging ? "松手即可加入队列" : "把文件拖到这里"}
                </span>
                <span className="text-[11px] opacity-70">
                  支持 .txt / .md / .srt，也可以点这里选择文件
                </span>
              </button>
            </div>
          ) : (
            <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
              {queue.map((item) => {
                const active = isActive(item.stage);
                return (
                  <li
                    key={item.path}
                    className="rounded-[10px] border px-2.5 py-2 transition-colors"
                    style={{
                      background: "var(--surface-muted)",
                      borderColor: active
                        ? "color-mix(in srgb, var(--brand) 55%, var(--border))"
                        : "var(--border)",
                    }}
                  >
                    <div className="flex items-center gap-2">
                      <span
                        className="min-w-0 flex-1 truncate text-[13px]"
                        title={item.path}
                      >
                        {item.name}
                      </span>

                      <span
                        className="shrink-0 text-[12px] tabular-nums"
                        style={{ color: stageColor(item.stage) }}
                      >
                        {STAGE_LABEL[item.stage]}
                        {item.stage === "translating" && item.segments > 0
                          ? ` ${item.done}/${item.segments}`
                          : ""}
                      </span>

                      {!running && (
                        <button
                          className="btn h-6 shrink-0 rounded-md border px-2 text-[11px]"
                          style={btnStyle}
                          onClick={() =>
                            setQueue((prev) => prev.filter((i) => i.path !== item.path))
                          }
                          title="从队列移除"
                        >
                          移除
                        </button>
                      )}
                    </div>

                    {active && item.segments > 0 && (
                      <div
                        className="mt-1.5 h-1 overflow-hidden rounded-full"
                        style={{ background: "var(--surface)" }}
                      >
                        <div
                          className="h-full rounded-full transition-[width] duration-200"
                          style={{
                            width: `${(item.done / item.segments) * 100}%`,
                            background: "var(--brand)",
                          }}
                        />
                      </div>
                    )}

                    {item.failedSegments > 0 && (
                      <p
                        className="m-0 mt-1 text-[11px]"
                        style={{ color: "var(--danger)" }}
                      >
                        {item.failedSegments} 个片段未能翻译，已保留原文
                      </p>
                    )}

                    {item.error && item.stage !== "done" && (
                      <p className="m-0 mt-1 text-[11px]" style={{ color: "var(--danger)" }}>
                        {item.error}
                      </p>
                    )}

                    {item.output && (
                      <p
                        className="m-0 mt-1 truncate text-[11px]"
                        style={{ color: "var(--text-muted)" }}
                        title={item.output}
                      >
                        → {item.output}
                      </p>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </section>

      {/* 操作区 */}
      <div className="flex items-center gap-2">
        <button
          className="btn h-10 rounded-lg border px-4 text-[13px] disabled:opacity-50"
          style={btnStyle}
          onClick={pick}
          disabled={running}
          title="选择要翻译的 txt / md / srt 文件"
        >
          添加文件
        </button>

        {/* 开始 / 停止共用一个键，与翻译页共用同一个组件，保证完全一致 */}
        <RunButton
          busy={running}
          load={load}
          label="开始翻译"
          disabled={!running && (!ready || queue.length === 0)}
          onClick={running ? stop : start}
        />

        {summary && (
          <span className="text-[12px]" style={{ color: "var(--text-muted)" }}>
            {summary.cancelled ? "已中断 · " : ""}
            成功 {summary.succeeded} · 失败 {summary.failed} · 跳过 {summary.skipped}
            {summary.failedSegments > 0 && ` · ${summary.failedSegments} 段保留原文`}
            {summary.qualitySegments > 0 &&
              (summary.qualityFixed >= summary.qualitySegments
                ? ` · 质检 ${summary.qualitySegments} 段均修正`
                : ` · 质检 ${summary.qualitySegments} 段，修正 ${summary.qualityFixed}`)}
            {" · "}
            {formatDuration(summary.elapsedMs)}
          </span>
        )}

        {error && (
          <span className="text-[12px]" style={{ color: "var(--danger)" }}>
            {error}
          </span>
        )}

        <button
          className="btn ml-auto h-10 rounded-lg border px-4 text-[13px] disabled:opacity-50"
          style={btnStyle}
          onClick={() => {
            setQueue([]);
            setSummary(null);
            setError("");
          }}
          disabled={running || queue.length === 0}
        >
          清空
        </button>
      </div>
    </div>
  );
}