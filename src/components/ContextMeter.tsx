import { useState } from "react";
import type { UsagePayload } from "../lib/ipc";

/** 1024 → 1.0K；参照 llama.cpp Web UI 的写法。 */
function fmt(n: number): string {
  if (n < 1000) return String(n);
  if (n < 10_000) return `${(n / 1000).toFixed(2).replace(/\.?0+$/, "")}K`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}K`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

interface Props {
  /** 最近一次补全的真实用量 */
  usage: UsagePayload;
  /** 本次会话累计（前端累加） */
  totals: { prompt: number; generated: number };
}

/**
 * 上下文用量面板，参照 llama.cpp Web UI：
 * 常驻显示一行进度，点开看本次/累计明细与平均速度。
 * 数据全部来自模型返回的真实 usage，不是估算。
 */
export function ContextMeter({ usage, totals }: Props) {
  const [open, setOpen] = useState(false);

  const used = usage.promptTokens + usage.completionTokens;
  const pct = Math.min(100, (used / usage.ctxSize) * 100);
  const remaining = Math.max(0, usage.ctxSize - used);
  const speed =
    usage.elapsedMs > 0
      ? usage.completionTokens / (usage.elapsedMs / 1000)
      : 0;

  const danger = pct > 85;

  return (
    <div className="shrink-0">
      <button
        className="flex w-full items-center gap-2 text-[11px]"
        onClick={() => setOpen((v) => !v)}
        title="查看 token 用量明细"
      >
        <span className="shrink-0 tabular-nums" style={{ color: "var(--text)" }}>
          上下文 <strong>{fmt(used)}</strong> / {fmt(usage.ctxSize)}
        </span>
        <div
          className="h-1 min-w-0 flex-1 overflow-hidden rounded-full"
          style={{ background: "var(--surface-muted)" }}
        >
          <div
            className="h-full rounded-full transition-[width] duration-200"
            style={{
              width: `${pct}%`,
              background: danger ? "var(--danger)" : "var(--brand)",
            }}
          />
        </div>
        <span
          className="shrink-0 tabular-nums"
          style={{ color: danger ? "var(--danger)" : "var(--text-muted)" }}
        >
          {pct.toFixed(0)}%
        </span>
        <span className="shrink-0" style={{ color: "var(--text-muted)" }}>
          {open ? "▾" : "▸"}
        </span>
      </button>

      {open && (
        <div
          className="mt-1.5 rounded-lg border p-2.5 text-[11px]"
          style={{
            background: "var(--surface)",
            borderColor: "var(--border)",
            color: "var(--text-muted)",
          }}
        >
          <div className="flex justify-between">
            <span>剩余</span>
            <span className="tabular-nums" style={{ color: "var(--text)" }}>
              {fmt(remaining)}
            </span>
          </div>

          <div
            className="my-1.5 h-px"
            style={{ background: "var(--border)" }}
          />

          <div className="mb-1">本次</div>
          <Row label="输入" value={`${usage.promptTokens} tok`} />
          <Row label="输出" value={`${usage.completionTokens} tok`} />
          <Row label="缓存命中" value={`${usage.cachedTokens} tok`} />

          <div className="mt-1.5 mb-1">本次会话累计</div>
          <Row label="输入" value={`${totals.prompt} tok`} />
          <Row label="输出" value={`${totals.generated} tok`} />

          <div
            className="my-1.5 h-px"
            style={{ background: "var(--border)" }}
          />
          <Row label="平均速度" value={`${speed.toFixed(1)} tok/s`} />
        </div>
      )}
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between">
      <span>{label}</span>
      <span className="tabular-nums" style={{ color: "var(--text)" }}>
        {value}
      </span>
    </div>
  );
}