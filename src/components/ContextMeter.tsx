import { useEffect, useRef, useState, type ReactNode } from "react";
import type { UsagePayload } from "../lib/ipc";

/** 1024 → 1.0K；对齐 llama.cpp Web UI 的写法。 */
function fmt(n: number): string {
  if (n < 1000) return String(n);
  if (n < 10_000) return `${(n / 1000).toFixed(2).replace(/\.?0+$/, "")}K`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}K`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}

/** 圆环半径与周长（viewBox 24，按钮 30px）。 */
const RADIUS = 9;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

interface Props {
  /** 最近一次补全的真实用量；还没发过请求时为 null */
  usage: UsagePayload | null;
  /** 当前模型的上下文上限（来自模型设置，不需要等请求） */
  ctxSize: number;
  /** 本次会话累计（前端累加） */
  totals: { prompt: number; generated: number };
}

/**
 * 上下文占用：一个圆环按钮，环本身就是进度条。
 * 点开是浮层明细，排版照 llama.cpp Web UI 的 Token usage details。
 */
export function ContextMeter({ usage, ctxSize, totals }: Props) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", close);
    return () => window.removeEventListener("mousedown", close);
  }, [open]);

  const ctx = usage?.ctxSize || ctxSize || 0;
  const used = usage ? usage.promptTokens + usage.completionTokens : 0;
  const pct = ctx > 0 ? Math.min(100, (used / ctx) * 100) : 0;
  const remaining = Math.max(0, ctx - used);
  const speed =
    usage && usage.elapsedMs > 0
      ? usage.completionTokens / (usage.elapsedMs / 1000)
      : 0;
  const danger = pct > 85;
  const color = danger ? "var(--danger)" : "var(--brand)";

  return (
    <div ref={ref} className="relative shrink-0">
      <button
        className="flex h-8 w-8 items-center justify-center rounded-full transition-colors"
        style={{ background: open ? "var(--surface-muted)" : "transparent" }}
        onClick={() => setOpen((v) => !v)}
        title={
          ctx > 0
            ? `上下文已用 ${pct.toFixed(0)}%（${fmt(used)} / ${fmt(ctx)}）`
            : "上下文用量"
        }
        aria-label="上下文用量"
        aria-expanded={open}
      >
        {/* 环本身表示占用比例，中心显示百分比 */}
        <svg
          className="absolute h-[27px] w-[27px]"
          viewBox="0 0 24 24"
          aria-hidden
        >
          <circle
            cx="12"
            cy="12"
            r={RADIUS}
            fill="none"
            stroke="var(--border)"
            strokeWidth="2.2"
          />
          {pct > 0 && (
            <circle
              cx="12"
              cy="12"
              r={RADIUS}
              fill="none"
              stroke={color}
              strokeWidth="2.2"
              strokeLinecap="round"
              strokeDasharray={CIRCUMFERENCE}
              strokeDashoffset={CIRCUMFERENCE * (1 - pct / 100)}
              transform="rotate(-90 12 12)"
              style={{ transition: "stroke-dashoffset 300ms" }}
            />
          )}
        </svg>
        <span
          className="relative text-[9px] font-medium leading-none tabular-nums"
          style={{ color: pct > 0 ? color : "var(--text-muted)" }}
        >
          {pct.toFixed(0)}
        </span>
      </button>

      {open && (
        <div
          className="popover absolute bottom-full right-0 z-30 mb-2 max-h-[70vh] w-[300px] max-w-[calc(100vw-24px)] overflow-y-auto p-3"
          style={{ animation: "pop-in-up var(--dur) var(--ease-out) both" }}
        >
          <div className="flex items-baseline gap-1.5 text-[13px]">
            <span style={{ color: "var(--text)" }}>上下文</span>
            <span style={{ color: "var(--text-muted)" }}>·</span>
            <span className="font-mono" style={{ color: "var(--text)" }}>
              {fmt(used)}
            </span>
            <span className="font-mono" style={{ color: "var(--text-muted)" }}>
              / {ctx > 0 ? fmt(ctx) : "—"}
            </span>
          </div>

          <div
            className="mt-2 h-[3px] w-full overflow-hidden rounded-full"
            style={{ background: "var(--surface-muted)" }}
          >
            <div
              className="h-full rounded-full"
              style={{ width: `${pct}%`, background: color }}
            />
          </div>

          <div className="mt-2 flex items-baseline justify-between text-[11px]">
            <span style={{ color: "var(--text-muted)" }}>{pct.toFixed(0)}% 已用</span>
            <span className="font-mono" style={{ color: "var(--text-muted)" }}>
              剩余 {fmt(remaining)}
            </span>
          </div>

          {!usage ? (
            <p
              className="m-0 mt-3 border-t pt-3 text-[12px]"
              style={{ borderColor: "var(--border)", color: "var(--text-muted)" }}
            >
              还没有发起过请求，用量数据会在第一次回复后出现。
            </p>
          ) : (
            <div
              className="mt-3 border-t pt-3"
              style={{ borderColor: "var(--border)" }}
            >
              <p className="m-0 text-[11px]" style={{ color: "var(--text-muted)" }}>
                用量明细
              </p>

              <div className="mt-3 flex flex-col gap-4">
                <Section title="本次会话累计">
                  <Detail label="输入已计算" value={`${totals.prompt} tok`} />
                  <Detail label="输出已生成" value={`${totals.generated} tok`} />
                </Section>

                <Section title="本次 · KV 缓存">
                  <Detail label="输入" value={`${usage.promptTokens} tok`} />
                  <Detail label="输出" value={`${usage.completionTokens} tok`} />
                  <Detail label="命中缓存" value={`${usage.cachedTokens} tok`} />
                  <div
                    className="mt-0.5 border-t pt-1"
                    style={{ borderColor: "var(--border)" }}
                  >
                    <div className="flex justify-between text-[12px]">
                      <span style={{ color: "var(--text-muted)" }}>KV 缓存合计</span>
                      <span
                        className="font-mono font-medium"
                        style={{ color: "var(--text)" }}
                      >
                        {used} tok
                      </span>
                    </div>
                  </div>
                </Section>

                <div
                  className="mt-1 border-t pt-1.5"
                  style={{ borderColor: "var(--border)" }}
                >
                  <Detail label="平均速度" value={`${speed.toFixed(1)} t/s`} />
                </div>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** 分组：小号大写标题 + 2px 行距。 */
function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div>
      <h3
        className="m-0 mb-2 text-[11px] font-medium uppercase tracking-wide"
        style={{ color: "var(--text-muted)", opacity: 0.7 }}
      >
        {title}
      </h3>
      <div className="flex flex-col gap-2">{children}</div>
    </div>
  );
}

/** 单行：标签在左，数值等宽字体右对齐。 */
function Detail({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between text-[12px]">
      <span style={{ color: "var(--text-muted)" }}>{label}</span>
      <span className="font-mono" style={{ color: "var(--text-muted)" }}>
        {value}
      </span>
    </div>
  );
}