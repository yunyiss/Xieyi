import type {
  AppInfo,
  RuntimeStatus,
  ServiceStatus,
  Settings,
} from "../lib/ipc";

interface Props {
  info: AppInfo | null;
  runtime: RuntimeStatus | null;
  service: ServiceStatus | null;
  settings: Settings | null;
}

function Dot({ on }: { on: boolean }) {
  return (
    <i
      className="inline-block size-2 shrink-0 rounded-full transition-colors"
      style={{
        background: on ? "var(--brand)" : "var(--border-strong)",
        boxShadow: on ? "0 0 0 3px color-mix(in srgb, var(--brand) 16%, transparent)" : "none",
      }}
    />
  );
}

/** 底部状态条：运行时 / 引擎 / 驻留 / 释放策略。 */
export function StatusBar({ info, runtime, service, settings }: Props) {
  const loaded = service?.loaded ?? [];
  const running = Boolean(service?.running);

  const ttlLabel =
    settings == null
      ? "—"
      : settings.ttlSecs === 0
        ? "不自动释放"
        : `空闲 ${settings.ttlSecs}s 释放`;

  const layersLabel =
    settings == null
      ? "—"
      : settings.gpuLayers === 1001
        ? "自动档"
        : settings.gpuLayers === 0
          ? "纯 CPU"
          : settings.gpuLayers >= 999
            ? "全 GPU"
            : `混合 ${settings.gpuLayers} 层`;

  return (
    <footer
      className="flex shrink-0 items-center gap-4 border-t px-4 py-2 text-[11px]"
      style={{
        borderColor: "var(--border)",
        color: "var(--text-muted)",
        background: "color-mix(in srgb, var(--surface) 70%, transparent)",
      }}
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

      <span className="truncate">
        {ttlLabel} · {layersLabel}
      </span>

      <span className="ml-auto shrink-0">
        {info ? `${info.os} / ${info.arch} · v${info.version}` : "读取环境…"}
      </span>
    </footer>
  );
}
