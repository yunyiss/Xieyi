import { useEffect, useRef, useState } from "react";
import type { ModelStatePayload } from "../lib/ipc";

/**
 * 一次加载完成后，引擎日志里能确认的“实际分配”。
 *
 * 与 `plan` 不同，这里全部是**实际值**：层数、上下文、KV 大小都来自
 * llama.cpp 自己打印的日志；解析不到时整个对象为 null，界面不显示。
 */
export interface LoadActual {
  model: string;
  gpuLayers: number;
  totalLayers: number;
  ctx: number | null;
  kvBytes: number | null;
  gpuModelBytes: number | null;
  cpuModelBytes: number | null;
  vramProjectedBytes: number | null;
  vramFreeBytes: number | null;
}

/** 从 `warm` / `loaded` 状态事件里提取实际分配；拿不到关键字段就返回 null。 */
export function toLoadActual(p: ModelStatePayload): LoadActual | null {
  if (p.state !== "warm" && p.state !== "loaded") return null;
  if (typeof p.gpuLayers !== "number" || typeof p.totalLayers !== "number") {
    return null;
  }
  return {
    model: p.model,
    gpuLayers: p.gpuLayers,
    totalLayers: p.totalLayers,
    ctx: typeof p.ctx === "number" ? p.ctx : null,
    kvBytes: typeof p.kvBytes === "number" ? p.kvBytes : null,
    gpuModelBytes: typeof p.gpuModelBytes === "number" ? p.gpuModelBytes : null,
    cpuModelBytes: typeof p.cpuModelBytes === "number" ? p.cpuModelBytes : null,
    vramProjectedBytes:
      typeof p.vramProjectedBytes === "number" ? p.vramProjectedBytes : null,
    vramFreeBytes: typeof p.vramFreeBytes === "number" ? p.vramFreeBytes : null,
  };
}

const GB = 1024 ** 3;

function fmtBytes(bytes: number | null): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return "—";
  const gb = bytes / GB;
  return gb >= 1
    ? `${gb.toFixed(1)} GB`
    : `${Math.max(1, Math.round(bytes / 1024 / 1024))} MB`;
}

function fmtCtx(ctx: number | null): string {
  if (ctx == null || ctx <= 0) return "—";
  return ctx >= 1024 ? `${Math.round(ctx / 1024)}K` : String(ctx);
}

interface Props {
  actual: LoadActual | null;
  /** 当前选中的模型；与 actual.model 不一致时不显示。 */
  model: string;
}

/**
 * 加载完成后的“实际分配”小标签。
 *
 * 正常时不占地方；点开才看详情与告警。文案刻意用书面语，避免界面上
 * 一会儿口语、一会儿术语。
 */
export function ModelLoadInfoChip({ actual, model }: Props) {
  const [open, setOpen] = useState(false);
  const [openUp, setOpenUp] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setOpen(false);
  }, [model]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (!rootRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  if (!actual || actual.model !== model) return null;

  const offloaded = actual.totalLayers - actual.gpuLayers;
  const noGpu = actual.gpuLayers === 0 && actual.totalLayers > 0;
  const partial = offloaded > 0;
  const tight =
    actual.vramProjectedBytes != null &&
    actual.vramFreeBytes != null &&
    actual.vramFreeBytes > 0 &&
    actual.vramProjectedBytes >= actual.vramFreeBytes * 0.95;

  const warnings: string[] = [];
  if (noGpu) {
    warnings.push(
      "本次加载未启用 GPU 加速，模型正在 CPU 上运行。请检查推理引擎与显卡驱动。",
    );
  } else if (partial) {
    warnings.push(
      `当前有 ${offloaded} 层驻留在系统内存，生成速度会低于全 GPU 运行。可尝试更小的量化或缩短上下文。`,
    );
  }
  if (tight) {
    warnings.push(
      "显存已接近上限，部分模型层可能回退到系统内存，生成速度可能明显下降。建议关闭占用显存的程序或减小上下文长度。",
    );
  }

  // 上下文已经由输入区左边的圆环展示，这里只标层数，避免两个地方重复。
  const label = `${actual.gpuLayers}/${actual.totalLayers} 层`;

  const toggle = () => {
    setOpen((prev) => {
      const next = !prev;
      if (next && rootRef.current) {
        const rect = rootRef.current.getBoundingClientRect();
        const below = window.innerHeight - rect.bottom;
        const above = rect.top;
        // 底部输入区上方空间有限时向下弹会被状态栏/窗口边缘切掉，
        // 自动改成向上展开。
        setOpenUp(below < 300 && above > below);
      }
      return next;
    });
  };

  return (
    <div ref={rootRef} className="relative shrink-0">
      <button
        type="button"
        className="rounded-full border px-2 py-0.5 text-[11px] tabular-nums transition-colors hover:bg-[var(--surface-muted)]"
        style={{ borderColor: "var(--border)", color: "var(--text-muted)" }}
        aria-expanded={open}
        aria-controls="load-actual-detail"
        title="查看本次加载的实际分配"
        onClick={toggle}
      >
        {label}
      </button>

      {open && (
        <div
          id="load-actual-detail"
          className={`absolute right-0 z-20 w-[300px] rounded-lg border p-3 text-[12px] shadow-lg ${
            openUp ? "bottom-full mb-2" : "top-full mt-2"
          }`}
          style={{
            borderColor: "var(--border)",
            background: "var(--surface)",
            color: "var(--text)",
          }}
          role="dialog"
          aria-label="实际分配"
        >
          <div className="mb-2 text-[12px] font-medium">本次加载的实际分配</div>

          <dl className="space-y-1.5">
            <Row k="显卡层数" v={`${actual.gpuLayers} / ${actual.totalLayers}`} />
            <Row k="上下文" v={fmtCtx(actual.ctx)} />
            <Row k="显存权重" v={fmtBytes(actual.gpuModelBytes)} />
            <Row k="内存权重" v={fmtBytes(actual.cpuModelBytes)} />
            <Row k="KV 缓存" v={fmtBytes(actual.kvBytes)} />
            {actual.vramProjectedBytes != null && (
              <Row
                k="显存预计 / 空闲"
                v={`${fmtBytes(actual.vramProjectedBytes)} / ${fmtBytes(actual.vramFreeBytes)}`}
              />
            )}
          </dl>

          {warnings.length > 0 && (
            <div className="mt-2 space-y-1.5">
              {warnings.map((w) => (
                <p
                  key={w}
                  className="rounded-md border px-2 py-1.5 leading-relaxed"
                  style={{
                    borderColor: "var(--border)",
                    background: "var(--surface-muted)",
                    color: "var(--text)",
                  }}
                >
                  {w}
                </p>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function Row({ k, v }: { k: string; v: string }) {
  return (
    <div className="flex items-baseline justify-between gap-3">
      <dt style={{ color: "var(--text-muted)" }}>{k}</dt>
      <dd className="tabular-nums">{v}</dd>
    </div>
  );
}
