interface Props {
  state: string;
}

/** 当前模型在引擎里的状态标签。 */
export function ModelStateBadge({ state }: Props) {
  const mapping: Record<string, { text: string; hot: boolean }> = {
    loaded: { text: "已加载", hot: true },
    loading: { text: "加载中", hot: true },
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
