import { useEffect, useRef, useState } from "react";
import type { Session } from "../lib/ipc";

interface Props {
  sessions: Session[];
  currentId: string;
  /** 当前会话标题；还没保存过的"新对话"不在 sessions 里，只能靠这个显示 */
  currentTitle: string;
  onPick: (id: string) => void;
  onNew: () => void;
  onDelete: (id: string) => void;
  onDeleteAll: () => void;
  disabled?: boolean;
}

/** 会话时间：今天只显示时分，更早显示月-日。 */
function stamp(secs: number): string {
  if (!secs) return "";
  const d = new Date(secs * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");

  return d.toDateString() === new Date().toDateString()
    ? `${pad(d.getHours())}:${pad(d.getMinutes())}`
    : `${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/**
 * 会话选择器：一个按钮 + 一张列表。
 *
 * 列表里可以直接删掉任意一条历史，不用先切过去再删；最后还有一键清空。
 * 上下文只存在本地，所以删除是彻底的，UI 上要让人看得见、点得到。
 */
export function SessionPicker({
  sessions,
  currentId,
  currentTitle,
  onPick,
  onNew,
  onDelete,
  onDeleteAll,
  disabled = false,
}: Props) {
  const [open, setOpen] = useState(false);
  /** 清空是破坏性操作，改成两段式：点一下变成"再点一次确认" */
  const [confirmClear, setConfirmClear] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) {
      setConfirmClear(false);
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

  return (
    <div ref={ref} className="relative min-w-0 max-w-[280px] flex-1">
      <button
        className="flex h-8 w-full min-w-0 items-center gap-1.5 rounded-lg border px-2.5 text-[12px] transition-colors disabled:cursor-not-allowed"
        style={{
          background: open ? "var(--surface-muted)" : "var(--surface)",
          borderColor: "var(--border)",
          color: "var(--text)",
        }}
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
        title="切换 / 删除历史对话"
        aria-expanded={open}
        aria-label="对话历史"
      >
        <span className="min-w-0 flex-1 truncate text-left">
          {currentTitle || "新对话"}
        </span>
        {sessions.length > 0 && (
          <span className="shrink-0 text-[11px]" style={{ color: "var(--text-muted)" }}>
            {sessions.length}
          </span>
        )}
        <svg
          className="h-3 w-3 shrink-0 transition-transform"
          style={{ transform: open ? "rotate(180deg)" : "none" }}
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
          className="popover absolute left-0 top-full z-30 mt-1.5 max-h-[60vh] w-[320px] max-w-[calc(100vw-24px)] overflow-y-auto p-1.5"
        >
          <button
            className="row flex min-h-10 w-full items-center gap-2 px-3 text-left text-[13px]"
            style={{ color: "var(--text)" }}
            onClick={() => {
              onNew();
              setOpen(false);
            }}
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden>
              <path
                d="M12 5v14M5 12h14"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
              />
            </svg>
            新建对话
          </button>

          {sessions.length > 0 && (
            <>
              <div className="my-1.5 h-px" style={{ background: "var(--border)" }} />
              {sessions.map((s) => {
                const active = s.id === currentId;
                return (
                  <div
                    key={s.id}
                    className="row group flex items-center pr-1"
                    data-selected={active}
                  >
                    <button
                      className="flex min-w-0 flex-1 items-center gap-2 px-3 py-2 text-left text-[13px]"
                      style={{ color: "inherit" }}
                      onClick={() => {
                        onPick(s.id);
                        setOpen(false);
                      }}
                      title={s.title}
                    >
                      <span className="min-w-0 flex-1 truncate">{s.title}</span>
                      <span
                        className="shrink-0 text-[11px] tabular-nums"
                        style={{ color: "var(--text-muted)", opacity: 0.6 }}
                      >
                        {stamp(s.updatedAt)}
                      </span>
                    </button>
                    <button
                      className="shrink-0 rounded px-1.5 py-1 text-[11px] opacity-0 transition-opacity group-hover:opacity-100 focus:opacity-100"
                      style={{ color: "var(--danger)" }}
                      onClick={() => onDelete(s.id)}
                      title="删除这条对话"
                      aria-label={`删除对话 ${s.title}`}
                    >
                      删除
                    </button>
                  </div>
                );
              })}
            </>
          )}

          {sessions.length === 0 && (
            <p
              className="m-0 px-3 py-2 text-[12px]"
              style={{ color: "var(--text-muted)" }}
            >
              还没有历史对话
            </p>
          )}

          <div className="my-1.5 h-px" style={{ background: "var(--border)" }} />

          <button
            className="row flex min-h-10 w-full items-center gap-2 px-3 text-left text-[13px] disabled:opacity-40"
            style={{ color: "var(--danger)" }}
            disabled={sessions.length === 0}
            onClick={() => {
              if (!confirmClear) {
                setConfirmClear(true);
                return;
              }
              onDeleteAll();
              setOpen(false);
            }}
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden>
              <path
                d="M4 7h16M10 11v6M14 11v6M6 7l1 12h10l1-12M9 7V5h6v2"
                stroke="currentColor"
                strokeWidth="1.7"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
            {confirmClear ? "再点一次，清空全部历史" : "清空全部历史"}
          </button>
        </div>
      )}
    </div>
  );
}
