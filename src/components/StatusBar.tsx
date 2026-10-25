import type { AppInfo } from "../lib/ipc";

interface Props {
  info: AppInfo | null;
}

/** 底部状态条：平台信息 / 运行时 / 驻留状态。 */
export function StatusBar({ info }: Props) {
  const ready = Boolean(info?.runtimeVersion);

  return (
    <footer
      className="flex shrink-0 items-center gap-4 border-t px-4 py-2 text-[12px]"
      style={{ borderColor: "var(--border)", color: "var(--text-muted)" }}
    >
      <span className="flex items-center gap-1.5">
        <i
          className="inline-block size-2 rounded-full"
          style={{ background: ready ? "var(--brand)" : "var(--border-strong)" }}
        />
        {ready ? `推理运行时 ${info?.runtimeVersion}` : "推理运行时未安装"}
      </span>

      <span className="flex items-center gap-1.5">
        <i
          className="inline-block size-2 rounded-full"
          style={{ background: "var(--border-strong)" }}
        />
        模型未加载 · 显存 0
      </span>

      <span className="ml-auto">
        {info ? `${info.os} / ${info.arch} · v${info.version}` : "读取环境…"}
      </span>
    </footer>
  );
}
