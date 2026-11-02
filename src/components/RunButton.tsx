import type { LoadState } from "./LoadIndicator";

interface Props {
  /** 本次任务是否在跑（翻译中 / 批量跑批中） */
  busy: boolean;
  /** 当前是否有模型在加载，含预热；用来区分「停止加载」和「停止」 */
  load: LoadState | null;
  /** 空闲时的文案，例如「开始翻译」 */
  label: string;
  disabled?: boolean;
  onClick: () => void;
}

/**
 * 「开始」与「停止」合用的主按钮，翻译页与批量页共用。
 *
 * 宽度固定，切到「停止」时按钮不会忽大忽小；忙时底色转 `--text-muted`，
 * 一眼能看出此刻点它是中断。
 */
export function RunButton({
  busy,
  load,
  label,
  disabled = false,
  onClick,
}: Props) {
  const text = busy
    ? load
      ? "停止加载"
      : "停止"
    : load
      ? load.state === "warming"
        ? "预热中…"
        : "加载中…"
      : label;

  return (
    <button
      className="flex h-10 min-w-[104px] shrink-0 items-center justify-center rounded-lg px-4 text-[13px] font-medium transition-colors disabled:opacity-50"
      /*
        悬停/离开必须回到**同一个**底色。早先离开时写的是 --brand，
        初始是 --accent-btn，两者不同 —— 鼠标划过一次按钮就永久变色了。

        文字色用变量而不是写死 #fff：深色模式下 --accent-btn 会反转成浅色，
        白字就看不见了。
      */
      style={{
        background: busy ? "var(--text-muted)" : "var(--accent-btn)",
        color: busy ? "var(--surface)" : "var(--accent-btn-text)",
      }}
      onMouseEnter={(e) => {
        if (!busy) e.currentTarget.style.background = "var(--accent-btn-hover)";
      }}
      onMouseLeave={(e) => {
        e.currentTarget.style.background = busy
          ? "var(--text-muted)"
          : "var(--accent-btn)";
      }}
      disabled={disabled}
      onClick={onClick}
      title={busy ? (load ? "停止加载模型" : "停止翻译") : label}
    >
      {text}
    </button>
  );
}
