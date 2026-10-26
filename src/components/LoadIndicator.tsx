import { useEffect, useState } from "react";
import type { ModelStatePayload } from "../lib/ipc";

/** 页面里保存的加载态：把后端上报的「已耗时」换算成本地时间轴起点。 */
export interface LoadState {
  model: string;
  state: "loading" | "warming";
  /** 本地时间轴起点（毫秒时间戳）。 */
  startAt: number;
  estimateMs: number;
}

/** 把一条模型状态事件转成 LoadState；非加载态返回 null。 */
export function toLoadState(p: ModelStatePayload): LoadState | null {
  if (p.state !== "loading" && p.state !== "warming") return null;
  return {
    model: p.model,
    state: p.state,
    startAt: Date.now() - (p.elapsedMs ?? 0),
    estimateMs: p.estimateMs ?? 0,
  };
}

interface Props {
  load: LoadState;
}

function formatSecs(ms: number): string {
  return (ms / 1000).toFixed(1);
}

/**
 * 加载进度条。llama.cpp 不提供真实进度，
 * 这里用「已耗时 / 按体积估算的总时长」近似，并明确标注为估算。
 */
export function LoadIndicator({ load }: Props) {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 200);
    return () => clearInterval(timer);
  }, []);

  const elapsed = Math.max(0, now - load.startAt);
  const eta = load.estimateMs - elapsed;

  const ratio =
    load.state === "warming"
      ? 0.94
      : load.estimateMs > 0
        ? Math.min(elapsed / load.estimateMs, 1)
        : 0;
  const percent = Math.min(ratio * 92, 92);

  const text =
    load.state === "warming"
      ? `预热中 ${formatSecs(elapsed)}s（首次推理较慢）`
      : eta > 0
        ? `加载中 ${formatSecs(elapsed)}s · 预计还需 ${formatSecs(eta)}s`
        : `加载中 ${formatSecs(elapsed)}s`;

  return (
    <div className="flex items-center gap-2.5">
      <span
        className="shrink-0 text-[12px] tabular-nums"
        style={{ color: "var(--text-muted)" }}
      >
        {text}
      </span>
      <div
        className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full"
        style={{ background: "var(--surface-muted)" }}
      >
        <div
          className="h-full rounded-full transition-[width] duration-200"
          style={{ width: `${percent}%`, background: "var(--brand)" }}
        />
      </div>
    </div>
  );
}