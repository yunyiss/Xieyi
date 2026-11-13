/**
 * 预设模型下载卡片：未下载 / 下载中 / 已暂停 / 已安装 四态。
 *
 * ## 为什么是四态
 *
 * 三态（未下载 / 下载中 / 已安装）漏掉了最要紧的一态：
 *
 * 用户点了「取消」之后，`.part` 是**故意保留**的（下次续传要用），
 * 那几个 GB 还在磁盘上。如果不显示成「已暂停」，用户眼里这个模型就
 * 回到了「未下载」——他会以为白下了，也可能再点一次从头开始。
 *
 * 更糟的是 `.part` 不进模型列表、启动时不扫描，界面上没有任何别的地方
 * 能看见它。所以这一态必须在这里，并且带一个明确的删除入口。
 *
 * ## 为什么进度订阅放在组件里
 *
 * 事件是全局的（一个 `preset://progress` 频道），但卡片是按模型的。
 * 订阅一次、按 `id` 分发到各自卡片，比每张卡各自 `listen` 更省——
 * 后者在引导层这种一次列三个模型的地方会开三个监听器。
 *
 * 卸载时必须注销：`useTauriEvent` 内部已经处理了 StrictMode 下的
 * 重复订阅问题（见该文件的注释）。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  cancelDownload,
  deletePresetPartial,
  downloadPreset,
  listPresets,
  onPresetProgress,
  type PresetProgress,
  type PresetStatus,
} from "../lib/ipc";
import { useTauriEvent } from "../lib/useTauriEvent";

/** 卡片当前处于哪个视觉状态。 */
type Phase =
  | { kind: "idle" }
  | { kind: "downloading"; downloaded: number; total: number }
  | { kind: "verifying"; total: number }
  | { kind: "paused"; downloaded: number; total: number }
  | { kind: "installed"; path: string };

/** 把一个 `PresetStatus` 映射到视觉状态。 */
function phaseOf(s: PresetStatus): Phase {
  if (s.downloaded) return { kind: "installed", path: s.path ?? "" };
  if (s.partialBytes > 0) {
    return {
      kind: "paused",
      downloaded: s.partialBytes,
      total: s.partialTotal || s.sizeBytes,
    };
  }
  return { kind: "idle" };
}

/** 字节 → 人类可读。用 1024 进制，与资源管理器一致。 */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  // GB 保留两位（4.31 GB），MB 及以下保留一位或无小数
  const digits = i >= 3 ? 2 : i === 2 ? 1 : 0;
  return `${v.toFixed(digits)} ${units[i]}`;
}

/** 百分比。总量未知时不猜——显示 0% 会让人以为卡住了。 */
export function percentOf(downloaded: number, total: number): number | null {
  if (!Number.isFinite(total) || total <= 0) return null;
  return Math.min(100, Math.max(0, Math.round((downloaded / total) * 100)));
}

interface RowProps {
  preset: PresetStatus;
  /** 这个模型的下载进度（由父组件按 id 分发） */
  progress: PresetProgress | null;
  /** 刷新列表（下载完成/删除后） */
  onRefresh: () => void;
  /** 全局 BUSY：同一时刻只允许一个下载，别的卡片要禁用 */
  busyId: string | null;
  setBusyId: (id: string | null) => void;
  /** 下载完成后要不要重启引擎。引导层不需要，设置抽屉需要 */
  onDownloaded?: () => void;
  /**
   * 紧凑模式：藏起说明文字，只留名称/体积/显存/按钮。
   *
   * 用在引导层展开的「其他档位」里——那时用户是在快速比较，
   * 不需要把三段产品文案再读一遍。
   */
  compact?: boolean;
}

function PresetRow({
  preset,
  progress,
  onRefresh,
  busyId,
  setBusyId,
  onDownloaded,
}: RowProps) {
  const [error, setError] = useState<string | null>(null);
  /** 本地覆盖的 phase：进度事件比 listPresets 刷新更及时 */
  const [live, setLive] = useState<Phase | null>(null);

  // 进度事件到达时更新 live；done/failed/cancelled 时让位给后端状态
  useEffect(() => {
    if (!progress || progress.id !== preset.id) return;

    if (progress.stage === "downloading") {
      setLive({
        kind: "downloading",
        downloaded: progress.downloaded,
        total: progress.total || preset.sizeBytes,
      });
    } else if (progress.stage === "verifying") {
      // 校验阶段：8 GB 文件要算约 7 秒，进度条不能停在 100% 不动
      setLive({ kind: "verifying", total: progress.total || preset.sizeBytes });
    } else if (progress.stage === "cancelled") {
      // 取消不是失败：交给 paused 态，不留红字
      setLive(null);
      setError(null);
    } else if (progress.stage === "failed") {
      setLive(null);
      setError(progress.error ?? "下载失败");
    } else if (progress.stage === "done") {
      setLive(null);
      setError(null);
    }
  }, [progress, preset.id, preset.sizeBytes]);

  const backend = phaseOf(preset);
  // 本地有临时态时优先（进度事件比列表刷新快），否则信后端
  const phase: Phase = live ?? backend;

  const start = useCallback(async () => {
    if (busyId) return;
    setError(null);
    setBusyId(preset.id);
    try {
      await downloadPreset(preset.id);
      onRefresh();
      onDownloaded?.();
    } catch (e) {
      setError(String(e));
      // 失败也要刷新：可能已留下 .part，界面要能显示成「已暂停」
      onRefresh();
    } finally {
      setBusyId(null);
      setLive(null);
    }
  }, [busyId, preset.id, setBusyId, onRefresh, onDownloaded]);

  const cancel = useCallback(async () => {
    try {
      await cancelDownload();
    } catch (e) {
      setError(String(e));
    }
    // 不在这里刷新：取消后后端要等传输真的停下来才释放，
    // 由进度事件的 cancelled 阶段触发一次刷新更准确
  }, []);

  const remove = useCallback(async () => {
    setError(null);
    try {
      await deletePresetPartial(preset.id);
      onRefresh();
    } catch (e) {
      setError(String(e));
    }
  }, [preset.id, onRefresh]);

  const pct =
    phase.kind === "downloading" || phase.kind === "paused"
      ? percentOf(phase.downloaded, phase.total)
      : phase.kind === "verifying"
        ? 100
        : null;

  return (
    <li
      className="rounded-[10px] border p-2.5"
      style={{
        background: "var(--surface-muted)",
        borderColor:
          phase.kind === "installed"
            ? "color-mix(in srgb, var(--brand) 45%, var(--border))"
            : "var(--border)",
      }}
    >
      {/* 标题行：名称 + 体积 */}
      <div className="flex items-baseline gap-2">
        <span className="min-w-0 flex-1 truncate text-[13px] font-medium">
          {preset.name}
        </span>
        <span
          className="shrink-0 text-[11px]"
          style={{ color: "var(--text-muted)" }}
          title={
            "建议显存。低于这个值时仍可用纯 CPU 运行，只是慢"
          }
        >
          {formatBytes(preset.sizeBytes)} · {preset.vramHintMb / 1024} GB 显存
        </span>
      </div>

      {/*
        显存与体积合并到标题行右侧（见上面的 span），说明文字只在
        **非紧凑**模式展示。

        紧凑模式用在引导层的「其他档位」里：那里用户已经决定要看
        其他选项，需要的是快速比较，不是读三遍产品文案。
      */}
      <p
        className="m-0 mt-1 text-[11px]"
        style={{ color: "var(--text-muted)" }}
      >
        {preset.summary}
      </p>

      {/* 进度条：下载中 / 校验中 显示 */}
      {(phase.kind === "downloading" || phase.kind === "verifying") && (
        <div className="mt-2">
          <div
            className="h-1 overflow-hidden rounded-full"
            style={{ background: "var(--border)" }}
          >
            <div
              className="h-full rounded-full transition-[width] duration-200"
              style={{
                width: `${pct ?? 0}%`,
                background: "var(--brand)",
              }}
            />
          </div>
          <div
            className="mt-1 flex items-center justify-between text-[10px]"
            style={{ color: "var(--text-muted)" }}
          >
            <span>
              {phase.kind === "verifying"
                ? "正在校验文件完整性…"
                : `${formatBytes(phase.downloaded)} / ${formatBytes(phase.total)}`}
            </span>
            <span>{pct != null ? `${pct}%` : "未知"}</span>
          </div>
        </div>
      )}

      {/* 已暂停：已下多少 + 可续传 */}
      {phase.kind === "paused" && (
        <div className="mt-2">
          <div
            className="h-1 overflow-hidden rounded-full"
            style={{ background: "var(--border)" }}
          >
            <div
              className="h-full rounded-full"
              style={{ width: `${pct ?? 0}%`, background: "var(--text-muted)" }}
            />
          </div>
          <p className="m-0 mt-1 text-[10px]" style={{ color: "var(--text-muted)" }}>
            已暂停：{formatBytes(phase.downloaded)}（可续传）
          </p>
        </div>
      )}

      {/* 操作行 */}
      <div className="mt-2 flex items-center gap-1.5">
        {phase.kind === "idle" && (
          <button
            className="btn h-7 rounded-md border px-2 text-[11px] disabled:opacity-50"
            style={{
              background: "var(--surface)",
              borderColor: "var(--border)",
              color: "var(--text)",
            }}
            onClick={start}
            disabled={busyId !== null}
          >
            下载
          </button>
        )}

        {phase.kind === "downloading" && (
          <button
            className="btn h-7 rounded-md border px-2 text-[11px]"
            style={{
              background: "var(--surface)",
              borderColor: "var(--border)",
              color: "var(--text)",
            }}
            onClick={cancel}
          >
            取消
          </button>
        )}

        {phase.kind === "verifying" && (
          <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
            下载完成，正在校验…
          </span>
        )}

        {phase.kind === "paused" && (
          <>
            <button
              className="btn h-7 rounded-md border px-2 text-[11px] disabled:opacity-50"
              style={{
                background: "var(--surface)",
                borderColor: "var(--border)",
                color: "var(--text)",
              }}
              onClick={start}
              disabled={busyId !== null}
            >
              继续下载
            </button>
            <button
              className="btn h-7 rounded-md border px-2 text-[11px]"
              style={{
                background: "transparent",
                borderColor: "var(--border)",
                color: "var(--text-muted)",
              }}
              onClick={remove}
              title="删掉已下载的部分，释放磁盘空间"
            >
              删除
            </button>
          </>
        )}

        {phase.kind === "installed" && (
          <span
            className="flex items-center gap-1 text-[11px]"
            style={{ color: "var(--brand-text)" }}
          >
            ✓ 已安装
          </span>
        )}
      </div>

      {error && (
        <p
          className="m-0 mt-1.5 text-[10px] leading-relaxed"
          style={{ color: "var(--danger)" }}
        >
          {error}
        </p>
      )}
    </li>
  );
}

interface Props {
  /**
   * 下载成功后是否重启推理引擎。
   *
   * `presets.ini` 是在**引擎启动时**按当时的目录扫描结果写出来的。
   * 新下的 `.gguf` 不重启引擎永远不会被路由看见——连"在设置里切过去"
   * 都做不到。所以：
   *
   * - 引导层：必须重启（用户正在配置，没什么可打断的）
   * - 设置抽屉：也要重启，但那里可能有模型 warm 着，重启会卸载它
   */
  onDownloaded?: () => void;
  /** 列表为空时显不显示。引导层空间紧，可以关掉 */
  emptyHint?: string;
  /**
   * 未下载的档位数量变化时回调。
   *
   * 设置抽屉用它实现「收起时透出还有几个没下」——否则推荐区收起来之后，
   * 用户完全不知道里面还有东西，也没法判断要不要展开。
   */
  onPendingChange?: (pending: number) => void;
  /**
   * 显示模式。**两处调用必须显式指定**，不设默认值——
   * 默认值会让"忘了选"看起来像选对了。
   *
   * - `full`：完整列表（设置抽屉用）。用户已经知道自己在配什么，
   *   需要看到所有选项
   * - `featured`：只显示一个推荐档，其余折叠（引导层用）。首次启动
   *   的用户面对的是"我要开始了"，不是"来比较一下三个量化档"
   */
  mode: "full" | "featured";
}

/**
 * `featured` 模式默认推荐哪一个。
 *
 * 选「均衡」（7B Q4_K_M）的理由：
 *
 * - 1.8B 质量在长句和术语上明显弱，作为"默认体验"不合适
 * - 7B Q8 要 12 GB 显存，绝大多数笔记本到不了
 * - 7B Q4 只要 8 GB，是 8G 显卡的甜点档；纯 CPU 也能跑（慢）
 *
 * 用 id 而不是下标：预设列表将来增删条目时下标会漂，id 不会。
 */
const FEATURED_ID = "Hy-MT2-7B-Q4_K_M";

/**
 * 预设模型下载列表。
 *
 * 挂在两处：首启引导层的模型卡片、设置抽屉的「模型与术语」页。
 * 两处的视觉密度差别很大，靠 `mode` 区分，**不共用同一套布局**。
 */
export function PresetModels({
  onDownloaded,
  emptyHint,
  mode,
  onPendingChange,
}: Props) {
  const [list, setList] = useState<PresetStatus[]>([]);
  const [loaded, setLoaded] = useState(false);
  /** 同一时刻只允许一个预设下载：后端有闸门，前端先禁用免得用户白点 */
  const [busyId, setBusyId] = useState<string | null>(null);
  /** `featured` 模式下是否展开了"其他档位" */
  const [expanded, setExpanded] = useState(false);

  /** 最近一次进度事件，按模型分发到卡片的 `progress` prop */
  const [byId, setById] = useState<Record<string, PresetProgress>>({});
  const aliveRef = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const next = await listPresets();
      if (aliveRef.current) setList(next);
    } catch {
      if (aliveRef.current) setList([]);
    } finally {
      if (aliveRef.current) setLoaded(true);
    }
  }, []);

  useEffect(() => {
    aliveRef.current = true;
    void refresh();
    return () => {
      aliveRef.current = false;
    };
  }, [refresh]);

  /**
   * 把「还有几个没下」报给父组件。
   *
   * 设置抽屉收起推荐区后，用户看不到里面有什么；靠这个数字在标题行
   * 透出「2 个未下载」，他才知道要不要展开。
   *
   * 用 `onPendingChange` 的 ref 包一层，避免父组件传内联箭头函数时
   * 每次渲染都触发这个 effect（那会让 setState 在父组件里反复跑）。
   */
  const pendingReporter = useRef(onPendingChange);
  pendingReporter.current = onPendingChange;
  const pending = list.filter((p) => !p.downloaded).length;
  useEffect(() => {
    if (loaded) pendingReporter.current?.(pending);
  }, [pending, loaded]);

  // 进度事件：只存最新的那条，按 id 覆盖
  useTauriEvent(onPresetProgress, (p) => {
    setById((prev) => ({ ...prev, [p.id]: p }));

    // 终态时刷新列表：`.part` 有没有留下、正式文件在不在，
    // 只有后端重新扫盘才知道
    if (p.stage === "done" || p.stage === "cancelled" || p.stage === "failed") {
      void refresh();
    }
  });

  if (!loaded) {
    return (
      <p className="m-0 text-[11px]" style={{ color: "var(--text-muted)" }}>
        正在读取…
      </p>
    );
  }

  if (list.length === 0) {
    return emptyHint ? (
      <p className="m-0 text-[11px]" style={{ color: "var(--text-muted)" }}>
        {emptyHint}
      </p>
    ) : null;
  }

  /** 渲染一张卡片。两处共用，避免行内逻辑重复。 */
  const row = (p: PresetStatus) => (
    <PresetRow
      key={p.id}
      preset={p}
      progress={byId[p.id] ?? null}
      onRefresh={refresh}
      busyId={busyId}
      setBusyId={setBusyId}
      onDownloaded={onDownloaded}
    />
  );

  if (mode === "full") {
    return (
      <ul className="m-0 flex list-none flex-col gap-2 p-0">
        {list.map((p) => row(p))}
      </ul>
    );
  }

  // ── featured：只显示一个推荐档，其余折叠 ──
  //
  // 引导层要回答"我要开始了，选哪个"，不是"来比较三个量化档"。
  // 三个卡片全展开会把首屏撑到必须滚动，而首次用户根本没有判断依据。
  //
  // 但**不能干脆不给选择**：纯 CPU 的用户下 7B 会慢到无法接受，
  // 所以要有一个明确的"其他档位"入口。
  const featured = list.find((p) => p.id === FEATURED_ID) ?? list[0];
  const others = list.filter((p) => p.id !== featured.id);

  return (
    <div className="flex flex-col gap-2">
      <ul className="m-0 flex list-none flex-col gap-2 p-0">
        {row(featured)}
      </ul>

      {/*
        展开的档位放在**按钮上方**，按钮压在最后。

        早先按钮夹在推荐卡和其余卡片之间，像一行标题把小节劈成两半，
        模型列表的连续性被打断了。放到底部，视觉上是"看完了这几个，
        还有别的"——顺序与阅读方向一致。
      */}
      {expanded && others.length > 0 && (
        <ul className="m-0 flex list-none flex-col gap-2 p-0">
          {others.map((o) => row(o))}
        </ul>
      )}

      {others.length > 0 && (
        <button
          className="btn flex h-8 items-center justify-center gap-1 rounded-md text-[11px]"
          style={{
            background: "transparent",
            color: "var(--text-muted)",
            border: "1px dashed var(--border)",
          }}
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
        >
          {/* 三角用 CSS 边框画，不引图标库——这个项目一贯不引重依赖 */}
          <span
            aria-hidden
            style={{
              display: "inline-block",
              width: 0,
              height: 0,
              borderLeft: "3.5px solid transparent",
              borderRight: "3.5px solid transparent",
              borderTop: expanded ? "none" : "4px solid currentColor",
              borderBottom: expanded ? "4px solid currentColor" : "none",
            }}
          />
          {expanded ? "收起" : `其他 ${others.length} 个档位`}
        </button>
      )}
    </div>
  );
}
