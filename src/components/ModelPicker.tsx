import { useEffect, useRef, useState, type ReactNode } from "react";
import { thinkingTiers, type RouterModel, type ThinkingMode, type ThinkingTier } from "../lib/ipc";

/** 只放文案；「需要多少上下文」「实际额度」全部来自后端的 `thinking_tiers`。 */
const TIER_LABEL: Record<ThinkingMode, { label: string; hint: string }> = {
  default: { label: "默认", hint: "模型决定" },
  off: { label: "关闭", hint: "" },
  low: { label: "低", hint: "思考" },
  medium: { label: "中", hint: "思考" },
  high: { label: "高", hint: "思考" },
  max: { label: "不限", hint: "可能很慢" },
};

const TIER_ORDER: ThinkingMode[] = [
  "default",
  "off",
  "low",
  "medium",
  "high",
  "max",
];

/** 没有后端数据时的兜底顺序，仅用于渲染顺序。 */
function fallbackTiers(): ThinkingTier[] {
  return TIER_ORDER.map((value) => ({ value, budgetTokens: null }));
}

/** 把 token 预算渲染成「最多 2,048 token」；不限档单独处理。 */
function budgetHint(budget: number | null): string {
  if (budget === null) return "";
  if (budget < 0) return "不限思考长度";
  return `最多 ${budget.toLocaleString()} token`;
}

/** 模型运行状态对应的小圆点颜色。 */
function stateDot(state: string): { color: string; label: string } {
  if (state === "loaded") return { color: "var(--brand)", label: "已加载" };
  if (state === "loading" || state === "warming")
    return { color: "var(--brand)", label: "加载中" };
  if (state === "unloading") return { color: "var(--text-muted)", label: "卸载中" };
  return { color: "transparent", label: "未加载" };
}

interface Props {
  models: RouterModel[];
  model: string;
  onModel: (id: string) => void;
  thinking: ThinkingMode;
  onThinking: (mode: ThinkingMode) => void;
  /** 当前模型的对话模板是否支持思考；false 时整个「思考强度」入口不显示 */
  thinkingSupported: boolean;
  disabled?: boolean;
}

/** 菜单层级：根菜单 → 模型列表 / 思考强度列表。 */
type View = "root" | "model" | "thinking";

/**
 * 模型与推理等级选择器。
 *
 * 交互照 DeepSeek / ChatGPT 的输入框：触发器只显示「模型名 + 当前档位」，
 * 点开是两级菜单 —— 根菜单列出「模型」「思考强度」，
 * 点进去换成对应列表。模型不支持思考时整行不出现。
 */
export function ModelPicker({
  models,
  model,
  onModel,
  thinking,
  onThinking,
  thinkingSupported,
  disabled = false,
}: Props) {
  const [open, setOpen] = useState(false);
  const [view, setView] = useState<View>("root");
  const [tiers, setTiers] = useState<ThinkingTier[]>([]);
  const ref = useRef<HTMLDivElement>(null);

  // 档位表由后端给，前端不另抄一份数字
  useEffect(() => {
    let alive = true;
    thinkingTiers()
      .then((list) => {
        if (alive) setTiers(list);
      })
      .catch(() => {
        if (alive) setTiers([]);
      });
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    if (!open) {
      setView("root");
      return;
    }
    const close = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", esc);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [open]);

  const current = models.find((m) => m.id === model);
  const dot = stateDot(current?.state ?? "unloaded");
  const shortModel = (model || "未发现模型").replace(/\.gguf$/i, "");
  const activeThinking = TIER_LABEL[thinking] ?? TIER_LABEL.off;
  // 「关闭」「默认」不占用触发器空间，只在菜单里体现
  const showThinkingTag =
    thinkingSupported && thinking !== "off" && thinking !== "default";

  return (
    <div ref={ref} className="relative min-w-0 max-w-[60vw]">
      <button
        className="flex min-w-0 max-w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-[12px] transition-colors disabled:cursor-not-allowed"
        style={{
          background: open ? "var(--surface-muted)" : "transparent",
          color: "var(--text-muted)",
          opacity: models.length === 0 ? 0.5 : 1,
        }}
        disabled={disabled || models.length === 0}
        onClick={() => setOpen((v) => !v)}
        title={current ? `当前模型：${current.id}` : "未发现模型"}
        aria-label="模型与思考强度"
        aria-expanded={open}
      >
        <span
          className="h-1.5 w-1.5 shrink-0 rounded-full"
          style={{
            background: dot.color,
            border: dot.color === "transparent" ? "1px solid var(--border)" : "none",
          }}
          title={dot.label}
        />
        <span className="min-w-0 truncate" style={{ color: "var(--text)" }}>
          {shortModel}
        </span>
        {showThinkingTag && (
          <span style={{ color: "var(--brand-text)" }}>{activeThinking.label}</span>
        )}
        <svg
          className="h-3 w-3 shrink-0 transition-transform"
          style={{ transform: open ? "rotate(180deg)" : "none" }}
          viewBox="0 0 24 24"
          fill="none"
          aria-hidden
        >
          <path
            d="M6 15l6-6 6 6"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </button>

      {open && (
        <div
          className="popover absolute bottom-full right-0 z-30 mb-2 max-h-[70vh] w-[300px] max-w-[calc(100vw-24px)] overflow-y-auto p-2"
          style={{ animation: "pop-in-up var(--dur) var(--ease-out) both" }}
        >
          {view === "root" && (
            <>
              <Row
                label="模型"
                value={shortModel}
                onClick={() => setView("model")}
              />
              {/* 不支持思考的模型直接不显示这一行 */}
              {thinkingSupported && (
                <Row
                  label="思考强度"
                  value={activeThinking.label}
                  onClick={() => setView("thinking")}
                />
              )}
            </>
          )}

          {view === "model" && (
            <>
              <Back title="模型" onClick={() => setView("root")} />
              <div className="flex flex-col">
                {models.map((m) => (
                  <Item
                    key={m.id}
                    selected={m.id === model}
                    label={m.id.replace(/\.gguf$/i, "")}
                    hint={stateDot(m.state).label}
                    onClick={() => {
                      onModel(m.id);
                      setOpen(false);
                    }}
                  />
                ))}
              </div>
            </>
          )}

          {view === "thinking" && (
            <>
              <Back title="思考强度" onClick={() => setView("root")} />
              <div className="flex flex-col">
                {(tiers.length > 0 ? tiers : fallbackTiers()).map((t) => {
                  const label = TIER_LABEL[t.value] ?? TIER_LABEL.off;
                  return (
                    <Item
                      key={t.value}
                      selected={t.value === thinking}
                      label={label.label}
                      hint={budgetHint(t.budgetTokens) || label.hint}
                      onClick={() => {
                        onThinking(t.value);
                        setOpen(false);
                      }}
                    />
                  );
                })}
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}

/** 根菜单的一行：左侧标题，右侧当前值 + 箭头。 */
function Row({
  label,
  value,
  onClick,
}: {
  label: string;
  value: string;
  onClick: () => void;
}) {
  return (
    <button
      className="row flex min-h-11 w-full items-center gap-3 px-3 text-left text-[14px]"
      style={{ color: "var(--text)" }}
      onClick={onClick}
    >
      <span className="shrink-0">{label}</span>
      <span
        className="ml-auto min-w-0 truncate text-[12px]"
        style={{ color: "var(--text-muted)" }}
      >
        {value}
      </span>
      <ChevronRight />
    </button>
  );
}

/** 列表里的可选项：左勾选、中间标题、右侧说明。 */
function Item({
  selected,
  label,
  hint,
  onClick,
}: {
  selected: boolean;
  label: string;
  hint: string;
  onClick: () => void;
}) {
  return (
    <button
      className="row flex min-h-11 w-full items-center gap-3 px-3 text-left text-[14px]"
      style={{ color: "var(--text)" }}
      data-selected={selected}
      onClick={onClick}
    >
      <span className="flex w-3 shrink-0 justify-center">
        {selected && (
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none">
            <path
              d="M5 12.5 10 17.5 19 7"
              stroke="currentColor"
              strokeWidth="2.4"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        )}
      </span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      <span
        className="shrink-0 text-[11px]"
        style={{ color: "var(--text-muted)", opacity: 0.6 }}
      >
        {hint}
      </span>
    </button>
  );
}

/** 二级列表的头部：返回箭头 + 标题。 */
function Back({ title, onClick }: { title: string; onClick: () => void }) {
  return (
    <button
      className="row flex min-h-9 w-full items-center gap-2 px-2 text-left text-[12px]"
      style={{ color: "var(--text-muted)" }}
      onClick={onClick}
    >
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden>
        <path
          d="M15 6l-6 6 6 6"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
      {title}
    </button>
  );
}

function ChevronRight(): ReactNode {
  return (
    <svg
      className="h-3.5 w-3.5 shrink-0"
      style={{ color: "var(--text-muted)" }}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden
    >
      <path
        d="M9 6l6 6-6 6"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}