import { useEffect, useRef, useState } from "react";
import type { ThemeMode } from "../lib/useTheme";

/**
 * 主题选择：点图标弹出三项菜单，一次点到目标。
 *
 * ## 为什么不做成「点一下换一下」
 *
 * 三态（跟随系统 / 浅色 / 深色）的循环按钮有个无法回避的问题：
 * 从「跟随系统」切到「深色」要点两次，且第三次会绕回来——
 * 用户无法直接指定想要的那个状态，只能靠记顺序。
 *
 * 主流做法有三种（参考 Kanso Protocol 的 theme-toggle 模式）：
 * 单图标循环、内联分段、下拉菜单。循环最省空间但最难用；
 * 内联分段最直观但占三倍宽度；下拉是折中——默认只占一个图标位，
 * 展开后三项平铺，点击即达。
 *
 * 顶栏空间紧张（右侧还有「设置」按钮），所以选下拉。
 */

const OPTIONS: { value: ThemeMode; label: string; hint: string }[] = [
  { value: "auto", label: "跟随系统", hint: "随系统的浅色/深色自动切换" },
  { value: "light", label: "浅色", hint: "始终使用浅色主题" },
  { value: "dark", label: "深色", hint: "始终使用深色主题" },
];

/** 当前模式的图标。 */
function ModeIcon({ mode }: { mode: ThemeMode }) {
  if (mode === "dark") {
    return (
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
        <path
          d="M20 14.5A8.5 8.5 0 0 1 9.5 4a8.5 8.5 0 1 0 10.5 10.5Z"
          stroke="currentColor"
          strokeWidth="1.7"
          strokeLinejoin="round"
        />
      </svg>
    );
  }
  if (mode === "light") {
    return (
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
        <circle cx="12" cy="12" r="4" stroke="currentColor" strokeWidth="1.7" />
        <path
          d="M12 2v2m0 16v2M2 12h2m16 0h2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4M19.1 4.9l-1.4 1.4M6.3 17.7l-1.4 1.4"
          stroke="currentColor"
          strokeWidth="1.7"
          strokeLinecap="round"
        />
      </svg>
    );
  }
  // auto：半实半虚的圆，表示「交给系统」
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
      <circle cx="12" cy="12" r="8" stroke="currentColor" strokeWidth="1.7" />
      <path d="M12 4a8 8 0 0 1 0 16Z" fill="currentColor" />
    </svg>
  );
}

interface Props {
  mode: ThemeMode;
  onChange: (mode: ThemeMode) => void;
}

export function ThemeMenu({ mode, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      // 点击菜单外部关闭。用 contains 而不是 stopPropagation，
      // 后者会和其它浮层的处理互相干扰。
      if (!boxRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const current = OPTIONS.find((o) => o.value === mode) ?? OPTIONS[0];

  return (
    // `ml-auto` 把整个容器推到顶栏右侧：它是 flex 布局里承接剩余空间的
    // 那一项，右侧的「设置」按钮紧随其后。加在容器而不是按钮上，
    // 否则浮层菜单（absolute 定位）会脱离这个对齐关系。
    <div ref={boxRef} className="relative ml-auto">
      <button
        onClick={() => setOpen((v) => !v)}
        className="btn flex h-8 w-8 items-center justify-center rounded-lg border"
        style={{
          background: "var(--surface)",
          borderColor: open ? "var(--brand)" : "var(--border)",
          color: open ? "var(--brand-text)" : "var(--text-muted)",
        }}
        title={`主题：${current.label}`}
        aria-label="选择主题"
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <ModeIcon mode={mode} />
      </button>

      {open && (
        <div
          role="menu"
          className="popover absolute right-0 z-30 mt-1.5 w-[172px] overflow-hidden rounded-xl border p-1"
          style={{
            background: "var(--surface)",
            borderColor: "var(--border)",
          }}
        >
          {OPTIONS.map((opt) => {
            const active = opt.value === mode;
            return (
              <button
                key={opt.value}
                role="menuitemradio"
                aria-checked={active}
                onClick={() => {
                  onChange(opt.value);
                  setOpen(false);
                }}
                className="row flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[12px]"
                data-selected={active}
                style={{ color: active ? "var(--brand-text)" : "var(--text)" }}
              >
                <span className="flex w-4 shrink-0 justify-center">
                  <ModeIcon mode={opt.value} />
                </span>
                <span className="flex-1">{opt.label}</span>
                {/* 选中打勾：只靠底色在浅色主题下不够醒目 */}
                {active && (
                  <svg
                    width="12"
                    height="12"
                    viewBox="0 0 24 24"
                    fill="none"
                    aria-hidden
                    style={{ color: "var(--brand)" }}
                  >
                    <path
                      d="m5 12 5 5L19 7"
                      stroke="currentColor"
                      strokeWidth="2.2"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    />
                  </svg>
                )}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
