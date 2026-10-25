import type { AppInfo, RuntimeStatus, ServiceStatus } from "../lib/ipc";

interface Props {
  info: AppInfo | null;
  runtime: RuntimeStatus | null;
  service: ServiceStatus | null;
}

function Dot({ on }: { on: boolean }) {
  return (
    <i
      className="inline-block size-2 rounded-full"
      style={{ background: on ? "var(--brand)" : "var(--border-strong)" }}
    />
  );
}

/** 底部状态条：运行时 / 引擎 / 驻留状态。 */
export function StatusBar({ info, runtime, service }: Props) {
  const loaded = service?.loaded ?? [];
  const running = Boolean(service?.running);

  return (
    <footer
      className="flex shrink-0 items-center gap-4 border-t px-4 py-2 text-[12px]"
      style={{ borderColor: "var(--border)", color: "var(--text-muted)" }}
    >
      <span className="flex items-center gap-1.5">
        <Dot on={Boolean(runtime?.installed)} />
        {runtime?.installed
          ? `运行时 ${runtime.version} · ${runtime.backend}`
          : "运行时未安装"}
      </span>

      <span className="flex items-center gap-1.5">
        <Dot on={running} />
        {running ? `引擎 ${service?.port ?? ""}` : "引擎未启动"}
      </span>

      <span className="flex items-center gap-1.5">
        <Dot on={loaded.length > 0} />
        {loaded.length > 0 ? `驻留 ${loaded.join(", ")}` : "模型未加载 · 显存 0"}
      </span>

      <span className="ml-auto">
        {info ? `${info.os} / ${info.arch} · v${info.version}` : "读取环境…"}
      </span>
    </footer>
  );
}
