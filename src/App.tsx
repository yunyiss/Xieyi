import { useCallback, useEffect, useState } from "react";
import { useTheme } from "./lib/useTheme";
import { ModelDrawer, type DrawerTab } from "./components/ModelDrawer";
import { ThemeMenu } from "./components/ThemeMenu";
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
  countLocalModels,
  type AppInfo,
  type RuntimeStatus,
  type ServiceStatus,
  type Settings,
} from "./lib/ipc";
import { computeReadiness } from "./lib/readiness";
import { shallowEqual } from "./lib/shallowEqual";

type Tab = "translate" | "batch" | "chat";

function App() {
  const [tab, setTab] = useState<Tab>("translate");
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [service, setService] = useState<ServiceStatus | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  /**
   * 抽屉打开时停在哪个页签。
   *
   * 用于「只缺模型」的场景：翻译页的提示按钮把用户直接送到模型页，
   * 不用他打开抽屉后再自己找。缺引擎才走首启引导。
   */
  const [drawerTab, setDrawerTab] = useState<DrawerTab>("general");
  /** 主题偏好：跟随系统 / 浅色 / 深色 */
  const { mode: themeMode, setMode: setThemeMode } = useTheme();
  /** 设置抽屉里改动后 +1，通知当前页面重新拉取模型与状态。 */
  const [refreshSignal, setRefreshSignal] = useState(0);
  /** 从翻译页带到对话页的草稿 */
  const [chatDraft, setChatDraft] = useState("");
  /**
   * 首次启动引导是否显示。
   *
   * `guideChecked` 为 false 表示还在判断：此时整个界面只铺底色、不渲染
   * 主界面，避免「先闪一下主界面再出引导」（见下面 return 前的门禁）。
   * 判断依据是"引擎没装"——缺模型不拦人。
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
        // 判定走 lib/readiness，与引导层、翻译页同一套口径。
        const rt = await getRuntimeStatus();
        if (!alive) return;
        if (!rt.installed) {
          setShowGuide(true);
          return;
        }
        // **用 countLocalModels 而不是 listRouterModels。**
        //
        // 后者内部是 `ensure()`——引擎没跑就会立刻 spawn 一个，把
        // 「双击图标」变成「拉起 llama-server」。实测窗口在 02:09:38
        // 就可见了，而引擎到 02:09:44 才起来，这 6 秒界面是空的，
        // 用户看到的就是"白屏一会儿"。
        //
        // 这里只需要知道"有没有模型"，扫目录就够了。引擎等用户真要
        // 翻译时再由 translate/chat/batch 各自 ensure，符合本项目
        // 「按需加载、不常驻占硬件」的定位。
        const n = await countLocalModels();
        if (!alive) return;
        const r = computeReadiness({ installed: rt.installed, modelCount: n });

        // **判据是 `canEnter`（只看引擎），不是 `ready`（还要有模型）。**
        //
        // 曾经写成 `!r.ready`，后果是：用户配好引擎、模型目录留空进去了，
        // 关掉应用重新打开，又被打回全屏引导层——因为"没有模型"又成立
        // 了一次。用户会以为配置丢了。
        //
        // 这与 `lib/readiness.ts` 里写明的设计是矛盾的：那里明确说
        // 「模型目录本来就允许空的……把这些用户挡在门外没有任何好处，
        // 而且会形成死循环」。启动判定没遵守它，等于同一个 bug 换了个
        // 位置又犯一次。
        //
        // 缺模型的提示交给翻译页（它本来就按缺失类型分流文案），
        // 不该在启动时把人拦在门外。
        setShowGuide(!r.canEnter);
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
        if (!alive) return;
        // 值没变就不 setState。
        //
        // 这三个 getter 每次返回的都是**新对象**，而 React 用引用比较 —
        // 直接 set 的话，哪怕数据一模一样，每 2.5 秒也会重渲染整棵树
        // （三个页面是常驻挂载的，不省着点会让空闲时的翻译页也跟着重画）。
        //
        // 这几个对象都很小且扁平，浅比较就够；比引入深比较库划算得多。
        setRuntime((prev) => (shallowEqual(prev, rt) ? prev : rt));
        setService((prev) => (shallowEqual(prev, sv) ? prev : sv));
        setSettings((prev) => (shallowEqual(prev, cfg) ? prev : cfg));
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

  /*
   * 引导判定完成前只铺底色，不画主界面。
   *
   * 少了这道门禁会先把翻译页画出来，等 `getRuntimeStatus` 回来后再盖上
   * 引导层。正常机器上只有几十毫秒、看不出来；但在慢机器（比如 x86 黑苹果
   * VM）上这段足够长，用户会看到「主界面闪一下，然后才出引导」。
   *
   * 底色与窗口底色（`titlebar::set_window_background` 用的 `--canvas`）一致，
   * 所以这段时间看起来只是内容稍晚出现，而不是画错了界面。
   */
  if (!guideChecked) {
    return <div className="h-full" style={{ background: "var(--canvas)" }} />;
  }

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

        {/* 主题选择：下拉菜单，一次点到目标状态 */}
        <ThemeMenu mode={themeMode} onChange={setThemeMode} />

        <button
          onClick={() => {
            setDrawerTab("general");
            setDrawerOpen(true);
          }}
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
            onOpenSetup={(need) => {
              if (need === "models") {
                // 只缺模型：就地打开设置抽屉的模型页，
                // 不把用户丢回全屏引导（那看起来像"重新来一遍"）
                setDrawerTab("models");
                setDrawerOpen(true);
              } else {
                // 缺引擎（或两样都缺）：走首启引导，那才是真正的从零配置
                setShowGuide(true);
              }
            }}
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
        initialTab={drawerTab}
        onClose={() => setDrawerOpen(false)}
        onChanged={() => setRefreshSignal((n) => n + 1)}
      />
    </div>
  );
}

export default App;
