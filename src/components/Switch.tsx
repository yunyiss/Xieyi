/**
 * 开关（布尔项）。
 *
 * 为什么不用原生 checkbox：勾选框是「选中若干项」的语义，而这里是
 * 一个独立的能力开关。开关的形态让「开 / 关」一眼可读，也不占两行。
 *
 * 性能：只动 `transform` 与 `background`，两者都走合成层，不触发重排。
 * 键盘可达：原生 button + `role="switch"`，空格/回车都能切换。
 */

interface Props {
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** 无障碍名称；界面上已有可见文字时也建议传，读屏才读得到 */
  label: string;
  disabled?: boolean;
}

export function Switch({ checked, onChange, label, disabled }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className="relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors duration-150 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 disabled:opacity-50"
      style={{
        background: checked
          ? "var(--brand)"
          : "color-mix(in srgb, var(--text-muted) 32%, transparent)",
        outlineColor: "var(--brand)",
        transitionDuration: "var(--dur-fast)",
      }}
    >
      <span
        className="inline-block h-4 w-4 rounded-full transition-transform"
        style={{
          transform: checked ? "translateX(16px)" : "translateX(2px)",
          background: "#fff",
          boxShadow: "var(--shadow-1)",
          transitionDuration: "var(--dur-fast)",
          transitionTimingFunction: "var(--ease-out)",
        }}
      />
    </button>
  );
}
