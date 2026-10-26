import { useEffect, useState } from "react";
import { ModelDrawer } from "./components/ModelDrawer";
import { StatusBar } from "./components/StatusBar";
import { Translate } from "./pages/Translate";
import { Chat } from "./pages/Chat";
import {
  getAppInfo,
  getRuntimeStatus,
  getServiceStatus,
  getSettings,
  type AppInfo,
  type RuntimeStatus,
  type ServiceStatus,
  type Settings,
} from "./lib/ipc";

type Tab = "translate" | "chat";

function App() {
  const [tab, setTab] = useState<Tab>("translate");
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [service, setService] = useState<ServiceStatus | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  /** 设置抽屉里改动后 +1，通知当前页面重新拉取模型与状态。 */
  const [refreshSignal, setRefreshSignal] = useState(0);

  useEffect(() => {
    getAppInfo()
      .then(setInfo)
      .catch((err) => console.error("读取环境信息失败", err));
  }, []);

  useEffect(() => {
    let alive = true;

    const refresh = async () => {
      try {
        const rt = await getRuntimeStatus();
        const sv = await getServiceStatus();
        const cfg = await getSettings();
        if (alive) {
          setRuntime(rt);
          setService(sv);
          setSettings(cfg);
        }
      } catch {
        // 引擎未启动时静默
      }
    };

    refresh();
    const timer = window.setInterval(refresh, 2500);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  const tabs: { key: Tab; label: string }[] = [
    { key: "translate", label: "翻译" },
    { key: "chat", label: "对话" },
  ];

  return (
    <div className="flex h-full flex-col">
      <header
        className="flex shrink-0 items-center gap-1 border-b px-4 py-2.5"
        style={{ borderColor: "var(--border)", background: "var(--surface)" }}
      >
        <span className="mr-4 text-[14px] font-semibold tracking-tight">
          LocalTranslate
        </span>

        {tabs.map((t) => {
          const active = tab === t.key;
          return (
            <button
              key={t.key}
              onClick={() => setTab(t.key)}
              className="rounded-lg px-3 py-1.5 text-[13px] transition-colors"
              style={{
                background: active ? "var(--brand-soft)" : "transparent",
                color: active ? "var(--brand-text)" : "var(--text-muted)",
                fontWeight: active ? 500 : 400,
              }}
            >
              {t.label}
            </button>
          );
        })}

        <button
          onClick={() => setDrawerOpen(true)}
          className="ml-auto flex h-8 shrink-0 items-center gap-1.5 rounded-lg border px-3 text-[12px] transition-colors"
          style={{
            background: "var(--surface)",
            borderColor: "var(--border)",
            color: "var(--text)",
          }}
          title="设置与模型"
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
            <path
              d="M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Z"
              stroke="currentColor"
              strokeWidth="1.7"
            />
            <path
              d="M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1.03 1.56V21a2 2 0 1 1-4 0v-.09A1.7 1.7 0 0 0 8.9 19.4a1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.7 1.7 0 0 0 4.6 15a1.7 1.7 0 0 0-1.56-1.03H3a2 2 0 1 1 0-4h.09A1.7 1.7 0 0 0 4.6 8.9a1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.7 1.7 0 0 0 9 4.6h.09A1.7 1.7 0 0 0 10.1 3.04V3a2 2 0 1 1 4 0v.09a1.7 1.7 0 0 0 1.03 1.56 1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.7 1.7 0 0 0 19.4 9v.09a1.7 1.7 0 0 0 1.56 1.03H21a2 2 0 1 1 0 4h-.09A1.7 1.7 0 0 0 19.4 15Z"
              stroke="currentColor"
              strokeWidth="1.7"
              strokeLinejoin="round"
            />
          </svg>
          设置
        </button>
      </header>

      <main className="min-h-0 flex-1">
        {tab === "translate" ? (
          <Translate refreshSignal={refreshSignal} />
        ) : (
          <Chat refreshSignal={refreshSignal} />
        )}
      </main>

      <StatusBar
        info={info}
        runtime={runtime}
        service={service}
        settings={settings}
      />

      <ModelDrawer
        open={drawerOpen}
        onClose={() => setDrawerOpen(false)}
        onChanged={() => setRefreshSignal((n) => n + 1)}
      />
    </div>
  );
}

export default App;
