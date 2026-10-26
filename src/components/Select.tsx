import { useEffect, useId, useRef, useState } from "react";

export interface SelectOption<T extends string | number> {
  value: T;
  label: string;
  /** 右侧灰色说明，例如「思考」「已加载」 */
  hint?: string;
}

interface Props<T extends string | number> {
  value: T;
  options: SelectOption<T>[];
  onChange: (value: T) => void;
  disabled?: boolean;
  /** 触发器上的 width / flex 类，例如 `w-[104px]`；不传则自适应宽度 */
  className?: string;
  /** 悬停提示 */
  title?: string;
  /** 菜单贴哪一边，默认左对齐 */
  align?: "left" | "right";
  /** 控件高度：sm 用于设置抽屉里的成对控件 */
  size?: "sm" | "md";
  placeholder?: string;
}

const SIZE = {
  sm: { box: "h-7 px-2 text-[12px]", item: "min-h-9 px-2.5 text-[13px]" },
  md: { box: "h-9 px-2.5 text-[13px]", item: "min-h-10 px-3 text-[13px]" },
} as const;

/**
 * 自定义下拉。
 *
 * 不用原生 `<select>` 是因为它的展开面板由系统绘制：灰底、直角色块、
 * 行高不受控，跟整套 UI 完全不搭。这里自己画，换来统一的圆角、阴影、
 * 悬停/选中反馈和一段 170ms 的入场动画。
 *
 * 面板只在展开时挂载，关闭时零成本；键盘操作与鼠标共用同一个高亮状态。
 */
export function Select<T extends string | number>({
  value,
  options,
  onChange,
  disabled = false,
  className = "",
  title,
  align = "left",
  size = "md",
  placeholder = "—",
}: Props<T>) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const ref = useRef<HTMLDivElement>(null);
  const listId = useId();

  const selectedIndex = options.findIndex((o) => o.value === value);
  const selected = selectedIndex >= 0 ? options[selectedIndex] : undefined;

  useEffect(() => {
    if (!open) return;

    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  // 打开时把高亮停在当前项上，键盘直接接着走
  useEffect(() => {
    if (open) setActive(selectedIndex >= 0 ? selectedIndex : 0);
  }, [open, selectedIndex]);

  const commit = (index: number) => {
    const next = options[index];
    if (!next) return;
    onChange(next.value);
    setOpen(false);
  };

  const onTriggerKey = (e: React.KeyboardEvent) => {
    if (disabled) return;

    if (!open) {
      if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
        e.preventDefault();
        setOpen(true);
      }
      return;
    }

    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        setActive((i) => Math.min(i + 1, options.length - 1));
        break;
      case "ArrowUp":
        e.preventDefault();
        setActive((i) => Math.max(i - 1, 0));
        break;
      case "Home":
        e.preventDefault();
        setActive(0);
        break;
      case "End":
        e.preventDefault();
        setActive(options.length - 1);
        break;
      case "Enter":
      case " ":
        e.preventDefault();
        commit(active);
        break;
      case "Tab":
        setOpen(false);
        break;
      default:
        break;
    }
  };

  return (
    <div ref={ref} className={`relative ${className}`}>
      <button
        type="button"
        className={`field flex w-full min-w-0 items-center gap-1.5 ${SIZE[size].box} disabled:opacity-50`}
        style={{ color: "var(--text)" }}
        data-open={open}
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
        onKeyDown={onTriggerKey}
        title={title}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
      >
        <span className={`min-w-0 flex-1 truncate text-left ${selected ? "" : "opacity-60"}`}>
          {selected?.label ?? placeholder}
        </span>
        <svg
          className="h-3 w-3 shrink-0 opacity-60 transition-transform"
          style={{
            transform: open ? "rotate(180deg)" : "none",
            transitionDuration: "var(--dur)",
          }}
          viewBox="0 0 24 24"
          fill="none"
          aria-hidden
        >
          <path
            d="M6 9l6 6 6-6"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </button>

      {open && (
        <div
          id={listId}
          role="listbox"
          className={`popover absolute top-full z-30 mt-1.5 max-h-[320px] min-w-full overflow-y-auto p-1 ${
            align === "right" ? "right-0" : "left-0"
          }`}
          style={{ width: "max-content", maxWidth: "min(320px, calc(100vw - 24px))" }}
        >
          {options.map((o, i) => {
            const isSelected = o.value === value;
            return (
              <button
                key={String(o.value)}
                type="button"
                role="option"
                aria-selected={isSelected}
                className={`row flex w-full items-center gap-2.5 text-left ${SIZE[size].item}`}
                style={{
                  color: isSelected ? "var(--brand-text)" : "var(--text)",
                }}
                data-selected={isSelected}
                data-active={i === active}
                onMouseEnter={() => setActive(i)}
                onClick={() => commit(i)}
              >
                <span className="flex w-3 shrink-0 justify-center">
                  {isSelected && (
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
                <span className="min-w-0 flex-1 truncate">{o.label}</span>
                {o.hint && (
                  <span
                    className="shrink-0 text-[11px]"
                    style={{ color: "var(--text-muted)", opacity: 0.7 }}
                  >
                    {o.hint}
                  </span>
                )}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
