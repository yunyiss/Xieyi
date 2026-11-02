import { useCallback, useEffect, useState } from "react";
import { useTheme } from "./lib/useTheme";
import { ModelDrawer } from "./components/ModelDrawer";
import { Onboarding } from "./components/Onboarding";
import { StatusBar } from "./components/StatusBar";
import { Translate } from "./pages/Translate";
import { Batch } from "./pages/Batch";
import { Chat } from "./pages/Chat";
import {
  getAppInfo,
  getRuntimeStatus,
  getServiceStatus,
  getSettings,
  listRouterModels,
  type AppInfo,
  type RuntimeStatus,
  type ServiceStatus,
  type Settings,
} from "./lib/ipc";

type Tab = "translate" | "batch" | "chat";

function App() {
  const [tab, setTab] = useState<Tab>("translate");
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [service, setService] = useState<ServiceStatus | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  /** 主题偏好：跟随系统 / 浅色 / 深色 */
  const { mode: themeMode, cycle: cycleTheme } = useTheme();
  /** 设置抽屉里改动后 +1，通知当前页面重新拉取模型与状态。 */
  const [refreshSignal, setRefreshSignal] = useState(0);
  /** 从翻译页带到对话页的草稿 */
  const [chatDraft, setChatDraft] = useState("");
  /**
   * 首次启动引导是否显示。
   *
   * `null` 表示还在判断（首帧不渲染，避免闪一下再消失）；
   * 判断依据是"引擎没装 或 没模型"——两者任一缺失就还处于未配置状态。
   */
  const [showGuide, setShowGuide] = useState(false);
  const [guideChecked, setGuideChecked] = useState(false);

  const handleSendToChat = useCallback((text: string) => {
    setChatDraft(text);
    setTab("chat");
  }, []);

  const handleDraftTaken = useCallback(() => setChatDraft(""), []);

  /** 运行时装好后三个页面都要重新拉模型，复用设置抽屉那条刷新信号。 */
  const handleRuntimeChanged = useCallback(
    () => setRefreshSignal((n) => n + 1),
    [],
  );

  useEffect(() => {
    getAppInfo()
      .then(setInfo)
      .catch((err) => console.error("读取环境信息失败", err));
  }, []);

  /**
   * 判断要不要弹首启引导 —— **只在启动时判断一次**。
   *
   * 之后不再重判：否则用户手动关掉引导后，只要引擎还没配好，
   * 下一次轮询就会把它又弹回来，等于关不掉。
   * 支持"先看看"的选择，所以这里只决定首次显示。
   */
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        // 引擎没装直接判定需要引导
        const rt = await getRuntimeStatus();
        if (!alive) return;
        if (!rt.installed) {
          setShowGuide(true);
          return;
        }
        // 引擎装了但一个模型都没有，同样算没配好
        const list = await listRouterModels();
        if (alive) setShowGuide(list.length === 0);
      } catch {
        // 拉不到状态就不打扰，免得误弹
      } finally {
        if (alive) setGuideChecked(true);
      }
    })();
    return () => {
      alive = false;
    };
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
    { key: "batch", label: "批量" },
    { key: "chat", label: "对话" },
  ];

  return (
    <div className="flex h-full flex-col">
      <header
        className="flex shrink-0 items-center gap-1 border-b px-4 py-2.5"
        style={{
          borderColor: "var(--border)",
          background: "var(--surface)",
          boxShadow: "var(--shadow-1)",
          zIndex: 20,
        }}
      >
        <span className="mr-4 flex items-center gap-2">
          {/*
            标题旁只放燕子本体（已抠掉深蓝底）。
            整张 logo 缩到 22px 时背景占了大半，主体只剩一个小点；
            裁掉背景、让主体填满画布后才认得出来。
          */}
          <img
            src="/swallow.png"
            alt=""
            width={26}
            height={26}
            className="shrink-0"
            aria-hidden
          />
          <span
            className="text-[14px] font-semibold tracking-tight"
            style={{ letterSpacing: "-0.01em" }}
          >
            Xieyi
            <span
              className="ml-1.5 text-[11px] font-normal tracking-normal"
              style={{ color: "var(--text-muted)" }}
            >
              写意
            </span>
          </span>
        </span>

        {tabs.map((t) => {
          const active = tab === t.key;
          return (
            <button
              key={t.key}
              onClick={() => setTab(t.key)}
              className="btn rounded-lg px-3 py-1.5 text-[13px]"
              style={{
                background: active ? "var(--brand-soft)" : "transparent",
                color: active ? "var(--brand-text)" : "var(--text-muted)",
                fontWeight: active ? 500 : 400,
              }}
              onMouseEnter={(e) => {
                if (!active) e.currentTarget.style.background = "var(--surface-muted)";
              }}
              onMouseLeave={(e) => {
                if (!active) e.currentTarget.style.background = "transparent";
              }}
            >
              {t.label}
            </button>
          );
        })}

        {/*
          主题切换：一键循环 跟随系统 → 浅色 → 深色。
          做成图标按钮而不是下拉，因为只有三态、切换成本极低。
        */}
        <button
          onClick={cycleTheme}
          className="btn ml-auto flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border"
          style={{
            background: "var(--surface)",
            borderColor: "var(--border)",
            color: "var(--text-muted)",
          }}
          title={
            themeMode === "auto"
              ? "主题：跟随系统（点击切换）"
              : themeMode === "light"
                ? "主题：浅色（点击切换）"
                : "主题：深色（点击切换）"
          }
          aria-label="切换主题"
        >
          {themeMode === "dark" ? (
            // 月亮：当前是深色
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
              <path
                d="M20 14.5A8.5 8.5 0 0 1 9.5 4a8.5 8.5 0 1 0 10.5 10.5Z"
                stroke="currentColor"
                strokeWidth="1.7"
                strokeLinejoin="round"
              />
            </svg>
          ) : themeMode === "light" ? (
            // 太阳：当前是浅色
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
              <circle cx="12" cy="12" r="4" stroke="currentColor" strokeWidth="1.7" />
              <path
                d="M12 2v2m0 16v2M2 12h2m16 0h2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4M19.1 4.9l-1.4 1.4M6.3 17.7l-1.4 1.4"
                stroke="currentColor"
                strokeWidth="1.7"
                strokeLinecap="round"
              />
            </svg>
          ) : (
            // 半圆：跟随系统
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" aria-hidden>
              <circle cx="12" cy="12" r="9" stroke="currentColor" strokeWidth="1.7" />
              <path d="M12 3a9 9 0 0 1 0 18Z" fill="currentColor" opacity="0.55" />
            </svg>
          )}
        </button>

        <button
          onClick={() => setDrawerOpen(true)}
          className="btn flex h-8 shrink-0 items-center gap-1.5 rounded-lg border px-3 text-[12px]"
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

      {/* 三个页面保持挂载，只切换显隐：
          切页不会丢掉翻译结果、聊天草稿和批量队列 */}
      <main className="min-h-0 flex-1">
        <div className={tab === "translate" ? "h-full" : "hidden"}>
          <Translate
            refreshSignal={refreshSignal}
            onSendToChat={handleSendToChat}
            onRuntimeChanged={handleRuntimeChanged}
            onOpenSetup={() => setShowGuide(true)}
          />
        </div>
        <div className={tab === "batch" ? "h-full" : "hidden"}>
          <Batch refreshSignal={refreshSignal} active={tab === "batch"} />
        </div>
        <div className={tab === "chat" ? "h-full" : "hidden"}>
          <Chat
            refreshSignal={refreshSignal}
            draft={chatDraft}
            onDraftTaken={handleDraftTaken}
          />
        </div>
      </main>

      {/*
        首启引导盖在最上层：三个页面始终是"配置好之后的样子"，
        配置过程不会嵌进任何业务界面。判断过之后才渲染，避免闪一下。
      */}
      {guideChecked && showGuide && (
        <Onboarding
          onDone={() => {
            setShowGuide(false);
            // 配好的结果要让三个页面都重新拉一次
            setRefreshSignal((n) => n + 1);
          }}
          onChanged={handleRuntimeChanged}
        />
      )}

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
