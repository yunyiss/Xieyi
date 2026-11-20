import { useEffect, useState } from "react";
import type { ModelStatePayload } from "../lib/ipc";

/** 页面里保存的加载态：把后端上报的「已耗时」换算成本地时间轴起点。 */
export interface LoadState {
  model: string;
  state: "loading" | "warming";
  /** 本地时间轴起点（毫秒时间戳）。 */
  startAt: number;
  estimateMs: number;
  /** 引擎日志解析出的阶段；老引擎/日志对不上时为 null。 */
  stage: LoadStage | null;
  percent: number | null;
  /** 估算的已读字节数（后端带“约”字展示）。 */
  readBytes: number | null;
  totalBytes: number | null;
  /** 估算的读取速度（字节/秒）。 */
  speedBps: number | null;
}

export type LoadStage = "prepare" | "read" | "cache" | "warmup" | "ready";

const STAGE_INDEX: Record<LoadStage, number> = {
  prepare: 0,
  read: 1,
  cache: 2,
  warmup: 3,
  ready: 4,
};

const STEPS: { key: LoadStage; label: string }[] = [
  { key: "prepare", label: "准备" },
  { key: "read", label: "读取模型" },
  { key: "cache", label: "分配缓存" },
  { key: "warmup", label: "预热" },
  { key: "ready", label: "完成" },
];

/** 把一条模型状态事件转成 LoadState；非加载态返回 null。 */
export function toLoadState(p: ModelStatePayload): LoadState | null {
  if (p.state !== "loading" && p.state !== "warming") return null;
  return {
    model: p.model,
    state: p.state,
    startAt: Date.now() - (p.elapsedMs ?? 0),
    estimateMs: p.estimateMs ?? 0,
    stage: normalizeStage(p.stage) ?? (p.state === "warming" ? "warmup" : null),
    percent: typeof p.percent === "number" ? p.percent : null,
    readBytes: typeof p.readBytes === "number" ? p.readBytes : null,
    totalBytes: typeof p.totalBytes === "number" ? p.totalBytes : null,
    speedBps: typeof p.speedBps === "number" ? p.speedBps : null,
  };
}

function normalizeStage(stage: string | undefined): LoadStage | null {
  if (
    stage === "prepare" ||
    stage === "read" ||
    stage === "cache" ||
    stage === "warmup" ||
    stage === "ready"
  ) {
    return stage;
  }
  return null;
}

const GB = 1024 ** 3;

/** 字节 → 人话。用 1024 进制，与资源管理器一致。 */
function fmtBytes(bytes: number | null): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes === 0) return "0 MB";
  const gb = bytes / GB;
  if (gb >= 1) return `${gb.toFixed(1)} GB`;
  return `${Math.max(1, Math.round(bytes / 1024 / 1024))} MB`;
}

function fmtSpeed(bps: number | null): string {
  if (bps == null || bps <= 0) return "";
  return bps >= 1024 * 1024
    ? `${(bps / 1024 / 1024).toFixed(0)} MB/s`
    : `${Math.max(1, Math.round(bps / 1024))} KB/s`;
}

function fmtSecs(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "";
  if (seconds >= 60) {
    const m = Math.floor(seconds / 60);
    const s = Math.round(seconds % 60);
    return `${m} 分 ${s} 秒`;
  }
  return `${Math.ceil(seconds)} 秒`;
}

interface Props {
  load: LoadState;
}

/**
 * 加载指示器。
 *
 * 默认只占一行：五段小进度条 + 当前阶段 + 读取信息；点「详情」才展开
 * 完整步骤。这样不会在每次加载时把下面的输入区顶下去。
 *
 * 后端拿不到阶段（老引擎 / 日志格式变了）时退回原来的“按体积估算”
 * 单条进度条，不会卡死。
 */
export function LoadIndicator({ load }: Props) {
  const [now, setNow] = useState(() => Date.now());
  const [open, setOpen] = useState(false);

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 200);
    return () => clearInterval(timer);
  }, []);

  const elapsed = Math.max(0, now - load.startAt);
  const stage = load.stage;
  const stageIndex = stage ? STAGE_INDEX[stage] : -1;
  const percent =
    load.percent != null
      ? load.percent
      : load.estimateMs > 0
        ? Math.min(92, Math.round((elapsed / load.estimateMs) * 92))
        : 0;

  // 读取阶段用“剩余字节 / 速度”算 ETA 更准；其它阶段退回按总时长估。
  const etaSecs =
    stage === "read" &&
    load.readBytes != null &&
    load.totalBytes != null &&
    load.speedBps != null &&
    load.speedBps > 0
      ? (load.totalBytes - load.readBytes) / load.speedBps
      : (load.estimateMs - elapsed) / 1000;

  const detail = (() => {
    if (!stage) return "";
    if (stage === "prepare") return "正在计算分层、准备启动参数";
    if (stage === "read") {
      const read = fmtBytes(load.readBytes);
      const total = fmtBytes(load.totalBytes);
      const speed = fmtSpeed(load.speedBps);
      return [
        read !== "—" ? `约 ${read} / ${total}` : "",
        speed,
        etaSecs > 0 ? `还需约 ${fmtSecs(etaSecs)}` : "",
      ]
        .filter(Boolean)
        .join(" · ");
    }
    if (stage === "cache") return "正在显卡上分配对话缓存";
    if (stage === "warmup") return "首次推理较慢，通常几秒到几十秒";
    return "即将就绪";
  })();

  const label = stage ? STEPS[stageIndex]?.label ?? "加载中" : "加载中";

  // 拿不到阶段：保留原来的单条进度条，至少还能看到“在动”。
  if (!stage) {
    return (
      <div className="flex items-center gap-2.5">
        <span
          className="shrink-0 text-[12px] tabular-nums"
          style={{ color: "var(--text-muted)" }}
        >
          {load.state === "warming"
            ? `预热中 ${(elapsed / 1000).toFixed(1)}s（首次推理较慢）`
            : `加载中 ${(elapsed / 1000).toFixed(1)}s`}
        </span>
        <div
          className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full"
          style={{ background: "var(--surface-muted)" }}
        >
          <div
            className="h-full rounded-full transition-[width] duration-200"
            style={{ width: `${load.state === "warming" ? 94 : percent}%`, background: "var(--brand)" }}
          />
        </div>
      </div>
    );
  }

  return (
    <div className="min-w-0">
      <div className="flex min-h-6 items-center gap-2.5 text-[12px]">
        <div
          className="flex shrink-0 items-center gap-1"
          role="progressbar"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={percent}
          aria-label={`模型加载进度，${label}`}
        >
          {STEPS.map((step, i) => {
            const done = i < stageIndex;
            const current = i === stageIndex;
            return (
              <span
                key={step.key}
                className={current ? "animate-pulse" : ""}
                style={{
                  width: 13,
                  height: 3,
                  borderRadius: 999,
                  background:
                    done || current ? "var(--brand)" : "var(--surface-muted)",
                  opacity: done ? 0.55 : 1,
                }}
              />
            );
          })}
        </div>

        <span className="shrink-0 font-medium" style={{ color: "var(--text)" }}>
          {label}
        </span>

        {detail && (
          <span
            className="min-w-0 truncate tabular-nums"
            style={{ color: "var(--text-muted)" }}
            title={detail}
          >
            {detail}
          </span>
        )}

        <button
          type="button"
          className="ml-auto shrink-0 rounded px-1.5 py-0.5 text-[11px] transition-colors hover:bg-[var(--surface-muted)]"
          style={{ color: "var(--text-muted)" }}
          aria-expanded={open}
          aria-controls="load-detail"
          onClick={() => setOpen((v) => !v)}
        >
          {open ? "收起" : `详情 · ${Math.round(percent)}%`}
        </button>
      </div>

      {open && (
        <div
          id="load-detail"
          className="mt-2 rounded-lg border p-3"
          style={{ borderColor: "var(--border)", background: "var(--surface)" }}
        >
          <div className="grid grid-cols-5 gap-2">
            {STEPS.map((step, i) => {
              const done = i < stageIndex;
              const current = i === stageIndex;
              return (
                <div key={step.key}>
                  <div
                    className="h-1 rounded-full"
                    style={{
                      background:
                        done || current ? "var(--brand)" : "var(--surface-muted)",
                      opacity: done ? 0.55 : 1,
                    }}
                  />
                  <div
                    className="mt-1.5 text-[11px]"
                    style={{
                      color: current ? "var(--text)" : "var(--text-muted)",
                      fontWeight: current ? 500 : 400,
                    }}
                  >
                    {step.label}
                  </div>
                </div>
              );
            })}
          </div>
          <div
            className="mt-2 text-[12px] leading-relaxed tabular-nums"
            style={{ color: "var(--text-muted)" }}
          >
            {detail || "正在加载模型…"}
          </div>
        </div>
      )}
    </div>
  );
}
