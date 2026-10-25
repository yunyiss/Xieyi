import { useEffect, useState } from "react";
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
      </header>

      <main className="min-h-0 flex-1">
        {tab === "translate" ? <Translate /> : <Chat />}
      </main>

      <StatusBar
        info={info}
        runtime={runtime}
        service={service}
        settings={settings}
      />
    </div>
  );
}

export default App;
