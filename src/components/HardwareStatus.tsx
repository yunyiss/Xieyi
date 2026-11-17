import type { MemoryInfo } from "../lib/ipc";

/**
 * 「通用」页的硬件状态。
 *
 * ## 为什么要有这一块
 *
 * 原来只有一句「当前可用显存 5136 MB · 自动档按模型体积分配层数」——
 * 它给的是数字和技术说明，用户看不出这意味着什么：
 * 我这台机器到底能跑多大的模型？显存不够时会怎样？
 *
 * 现在把显存和内存都画成「总量 / 可用 + 已用百分比」，并说清
 * 「模型优先放显存，放不下的层由内存里的 CPU 跑」。
 *
 * ## 为什么数据由父组件传入
 *
 * 这块曾经自己发探测请求。结果是**每次从别的页签切回来都会重新挂载、
 * 重新探测**，界面上先闪一下「检测中…」。
 *
 * 现在探测放在 `ModelDrawer`（长期挂载）里，这里只负责显示；切页签不再闪。
 * 内存总量 / 可用量由后端 `sysinfo` 一次返回，口径与 LocModel 一致。
 *
 * `undefined` 表示**还没读过**（显示「检测中…」），`null` 表示
 * **读过但读不到**（显示「未检测到 / 读取不到」）。两者不能混为一谈。
 */

interface Props {
  /** 当前可用显存（MB）；`null` 表示读不到 */
  freeVramMb: number | null;
  /** 总显存（MB）；`undefined` = 尚未探测，`null` = 探测过但无结果 */
  totalVramMb: number | null | undefined;
  /** 内存总量与可用量；语义同上 */
  ram: MemoryInfo | null | undefined;
  /** 重新探测硬件 */
  onRedetect: () => void;
}

/** MB → GB，保留一位小数。 */
function gb(mb: number): string {
  return `${(mb / 1024).toFixed(1)} GB`;
}

/** 已用百分比：任一侧未知返回 `null`，不画条。 */
function usedPercent(total: number | null, available: number | null): number | null {
  if (total == null || available == null || total <= 0) return null;
  const used = Math.max(0, total - available);
  return Math.min(100, Math.round((used / total) * 100));
}

function Bar({ percent }: { percent: number }) {
  return (
    <div
      className="mt-1.5 h-1.5 overflow-hidden rounded-full"
      style={{ background: "var(--border)" }}
      aria-hidden
    >
      <div
        className="h-full rounded-full transition-[width] duration-200"
        style={{
          width: `${percent}%`,
          background: "color-mix(in srgb, var(--brand) 60%, var(--border))",
        }}
      />
    </div>
  );
}

export function HardwareStatus({
  freeVramMb,
  totalVramMb,
  ram,
  onRedetect,
}: Props) {
  const loading = totalVramMb === undefined || ram === undefined;

  const vramPercent = usedPercent(totalVramMb ?? null, freeVramMb);
  const vramText =
    totalVramMb === undefined
      ? "检测中…"
      : totalVramMb == null
        ? "未检测到独立显卡"
        : freeVramMb == null
          ? `${gb(totalVramMb)} · 可用未知`
          : `${gb(totalVramMb)} · 可用 ${gb(freeVramMb)}`;

  const ramPercent = usedPercent(ram?.totalMb ?? null, ram?.availableMb ?? null);
  const ramText =
    ram === undefined
      ? "检测中…"
      : ram == null
        ? "读取不到"
        : `${gb(ram.totalMb)} · 可用 ${gb(ram.availableMb)}`;

  return (
    <section className="mb-4">
      <div className="mb-1.5 flex items-baseline gap-2">
        <span className="section-title">硬件</span>
        <button
          className="btn ml-auto h-6 rounded-md border px-1.5 text-[10px] transition-colors disabled:opacity-50"
          style={{
            background: "var(--surface)",
            borderColor: "var(--border)",
            color: "var(--text-muted)",
          }}
          onClick={onRedetect}
          disabled={loading}
          title="重新读取显存与内存"
        >
          {loading ? "检测中…" : "重新检测"}
        </button>
      </div>

      <div
        className="rounded-[10px] border p-2.5"
        style={{
          background: "var(--surface-muted)",
          borderColor: "var(--border)",
        }}
      >
        <div className="flex items-baseline gap-2 text-[12px]">
          <span
            className="w-[44px] shrink-0"
            style={{ color: "var(--text-muted)" }}
          >
            显存
          </span>
          <span className="min-w-0 flex-1 truncate tabular-nums" title={vramText}>
            {vramText}
          </span>
          {vramPercent != null && (
            <span
              className="shrink-0 text-[10px] tabular-nums"
              style={{ color: "var(--text-muted)" }}
            >
              已用 {vramPercent}%
            </span>
          )}
        </div>
        {vramPercent != null && <Bar percent={vramPercent} />}

        <div className="mt-2 flex items-baseline gap-2 text-[12px]">
          <span
            className="w-[44px] shrink-0"
            style={{ color: "var(--text-muted)" }}
          >
            内存
          </span>
          <span className="min-w-0 flex-1 truncate tabular-nums" title={ramText}>
            {ramText}
          </span>
          {ramPercent != null && (
            <span
              className="shrink-0 text-[10px] tabular-nums"
              style={{ color: "var(--text-muted)" }}
            >
              已用 {ramPercent}%
            </span>
          )}
        </div>
        {ramPercent != null && <Bar percent={ramPercent} />}

        <p
          className="m-0 mt-1.5 text-[10px] leading-relaxed"
          style={{ color: "var(--text-muted)" }}
        >
          {totalVramMb === null
            ? "没有可用显存信息，将由 CPU 运行；速度取决于处理器。"
            : "模型优先放显存；放不下的层会留在内存里由 CPU 跑，速度会下降。"}
        </p>
      </div>
    </section>
  );
}
