interface Props {
  state: string;
}

/** 模型状态的中文标签，下拉里和角标共用一份。 */
export const STATE_LABEL: Record<string, string> = {
  loaded: "已加载",
  loading: "加载中",
  warming: "预热中",
  unloading: "卸载中",
  unloaded: "未加载",
  missing: "未加载",
};

export function stateLabel(state: string): string {
  return STATE_LABEL[state] ?? "未加载";
}

/** 当前模型在引擎里的状态标签。 */
export function ModelStateBadge({ state }: Props) {
  const mapping: Record<string, { text: string; hot: boolean }> = {
    loaded: { text: "已加载", hot: true },
    loading: { text: "加载中", hot: true },
    warming: { text: "预热中", hot: true },
    unloading: { text: "卸载中", hot: true },
    unloaded: { text: "未加载", hot: false },
    missing: { text: "未加载", hot: false },
  };
  const info = mapping[state] ?? { text: "未加载", hot: false };

  return (
    <span
      className="shrink-0 rounded-full px-2 py-0.5 text-[11px]"
      style={{
        background: info.hot ? "var(--brand-soft)" : "transparent",
        color: info.hot ? "var(--brand-text)" : "var(--text-muted)",
        border: info.hot ? "none" : "1px solid var(--border)",
      }}
    >
      {info.text}
    </span>
  );
}
