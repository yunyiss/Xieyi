import { useCallback, useEffect, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { LoadIndicator, toLoadState, type LoadState } from "../components/LoadIndicator";
import { ModelStateBadge } from "../components/ModelStateBadge";
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

  // 批量进度
  useEffect(() => {
    let un: UnlistenFn | null = null;
    onBatchProgress((p) => {
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
    }).then((fn) => (un = fn));
    return () => {
      un?.();
    };
  }, []);

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
      .then((fn) => (un = fn));
    return () => {
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
        <select
          className={`${selectClass} w-[104px] shrink-0`}
          style={selectStyle}
          value={source}
          onChange={(e) => setSource(e.target.value)}
          disabled={running}
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
          disabled={running}
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
          disabled={models.length === 0 || running}
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

      {/* 队列 */}
      <section
        className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border"
        style={{
          background: dragging ? "var(--brand-soft)" : "var(--surface)",
          borderColor: dragging ? "var(--brand)" : "var(--border)",
        }}
      >
        <header
          className="flex shrink-0 items-center justify-between border-b px-3 py-2 text-[12px]"
          style={{ borderColor: "var(--border)", color: "var(--text-muted)" }}
        >
          <span>
            队列 {queue.length} 个文件
            {queue.length > 0 && ` · 已完成 ${finished} · 失败 ${failed}`}
          </span>
          <span>
            {dragging
              ? "松手即可加入队列"
              : "把 txt / md / srt 拖进来，或点「添加文件」"}
          </span>
        </header>

        <div className="min-h-0 flex-1 overflow-auto p-2">
          {queue.length === 0 ? (
            <div className="flex h-full items-center justify-center">
              <p className="m-0 text-[13px]" style={{ color: "var(--text-muted)" }}>
                队列是空的
              </p>
            </div>
          ) : (
            <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
              {queue.map((item) => {
                const active = isActive(item.stage);
                return (
                  <li
                    key={item.path}
                    className="rounded-lg border px-2.5 py-2"
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
                          className="h-6 shrink-0 rounded-md border px-2 text-[11px]"
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
          className="h-10 rounded-lg border px-4 text-[13px] disabled:opacity-50"
          style={btnStyle}
          onClick={pick}
          disabled={running}
        >
          添加文件
        </button>

        {/* 开始与停止合用一个键：跑起来后变成「停止」，和翻译页、对话页一致 */}
        <button
          className="h-10 rounded-lg px-5 text-[13px] font-medium text-white transition-colors disabled:opacity-50"
          style={{ background: running ? "var(--text-muted)" : "var(--brand)" }}
          onMouseEnter={(e) => {
            if (!running) e.currentTarget.style.background = "var(--brand-hover)";
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = running
              ? "var(--text-muted)"
              : "var(--brand)";
          }}
          onClick={running ? stop : start}
          disabled={!running && (!ready || queue.length === 0)}
        >
          {running
            ? load
              ? "停止加载"
              : "停止"
            : load
              ? load.state === "warming"
                ? "预热中…"
                : "加载模型中…"
              : "开始翻译"}
        </button>

        {summary && (
          <span className="text-[12px]" style={{ color: "var(--text-muted)" }}>
            {summary.cancelled ? "已中断 · " : ""}
            成功 {summary.succeeded} · 失败 {summary.failed} · 跳过 {summary.skipped}
            {summary.failedSegments > 0 && ` · ${summary.failedSegments} 段保留原文`}
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
          className="ml-auto h-10 rounded-lg border px-4 text-[13px] disabled:opacity-50"
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