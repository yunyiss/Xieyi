import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { LoadIndicator, toLoadState, type LoadState } from "../components/LoadIndicator";
import { ModelStateBadge, stateLabel } from "../components/ModelStateBadge";
import { QualityHint } from "../components/QualityHint";
import { RunButton } from "../components/RunButton";
import { Select } from "../components/Select";
import { SOURCE_LANGUAGES, TARGET_LANGUAGES } from "../lib/languages";
import { computeReadiness } from "../lib/readiness";
import { mergeModelStates } from "../lib/modelState";
import {
  abort,
  getRuntimeStatus,
  getServiceStatus,
  listLocalModels,
  newRequestId,
  onModelState,
  onQualityReport,
  onToken,
  onTranslateProgress,
  restartService,
  setModelsDir,
  translate,
  type QualityReport,
  type RouterModel,
  type RuntimeStatus,
  type ServiceStatus,
} from "../lib/ipc";
import { useTauriEvent } from "../lib/useTauriEvent";

const LANGUAGES = SOURCE_LANGUAGES;
const TARGETS = TARGET_LANGUAGES;

const selectStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};


/** 质检提示：平时只是一个数字，鼠标悬停才展开细节，不打扰正常翻译。 */
function QualityNote({
  reports,
  total,
}: {
  reports: QualityReport[];
  /** 本次翻译的总片段数，用来显示「2/12 段」而不是孤零零一个数字 */
  total: number;
}) {
  const flagged = reports.length;
  const unfixed = reports.filter((r) => !r.fixed).length;
  const scope = total > 0 ? `/${total}` : "";
  const text =
    unfixed > 0
      ? `质检 ${flagged}${scope} 段 · ${unfixed} 段仍可疑`
      : `质检 ${flagged}${scope} 段 · 均已修正`;

  const detail = reports
    .map(
      (r) =>
        `「${r.segment}…」${r.found}${r.fixed ? "（已修正）" : "（重试后仍可疑）"}`,
    )
    .join("\n");

  return (
    <span
      className="min-w-0 truncate"
      style={{ color: unfixed > 0 ? "var(--danger)" : "var(--brand-text)" }}
      title={detail}
    >
      {text}
    </span>
  );
}

interface Props {
  /** 设置抽屉里改动后递增，用于触发重新扫描。 */
  refreshSignal?: number;
  /** 把译文送到对话页继续调整语意。 */
  onSendToChat?: (text: string) => void;
  /** 运行时安装/切换成功后通知父级，让常驻挂载的其他页面也重新拉取模型。 */
  onRuntimeChanged?: () => void;
  /**
   * 打开配置入口。
   *
   * 参数是**缺什么**，由父级决定去哪：
   * - 缺引擎 → 走首启引导（真正的从零配置）
   * - 只缺模型 → 打开设置抽屉的模型页（就地补，不用全屏重来）
   */
  onOpenSetup?: (need: "engine" | "models") => void;
}

export function Translate({
  refreshSignal = 0,
  onSendToChat,
  onRuntimeChanged,
  onOpenSetup,
}: Props) {
  const [source, setSource] = useState("auto");
  const [target, setTarget] = useState("zh");
  const [input, setInput] = useState("");
  const [output, setOutput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [service, setService] = useState<ServiceStatus | null>(null);
  const [models, setModels] = useState<RouterModel[]>([]);
  const [model, setModel] = useState("");
  const [engineError, setEngineError] = useState("");
  const [load, setLoad] = useState<LoadState | null>(null);
  const [segmentProgress, setSegmentProgress] = useState({
    done: 0,
    total: 0,
  });
  /** 本次翻译里被质检挑出问题、已自动重试的片段 */
  const [quality, setQuality] = useState<QualityReport[]>([]);
  /**
   * 当前译文是否整段来自缓存。
   *
   * 只用于显示「重新翻译」按钮的可用性判断：
   * 命中缓存时按钮更有意义（用户按下去才可能得到不同结果）；
   * 没命中时也能点，只是效果等于再跑一次。
   */
  const [fromCache, setFromCache] = useState(false);
  /**
   * 瞬时提示文案。`null` 表示不显示。
   *
   * 用完即消失，不常住——用户只需要知道「这次是缓存」，
   * 不需要它一直占着标题栏。定时器在组件卸载时清掉。
   */
  const [toast, setToast] = useState<string | null>(null);
  const toastTimer = useRef<number | null>(null);

  const unlistenRef = useRef<UnlistenFn | null>(null);

  /**
   * 准备模型列表：**只扫目录，不启动引擎**。
   *
   * 早先这里是「getRuntimeStatus → startService → listRouterModels」，
   * 也就是页面一挂载就把 llama-server 拉起来。三个页面都是常驻挂载的，
   * 于是"双击图标"等于"立即启动推理进程"——实测日志里窗口在
   * 02:09:38 就可见了，引擎到 02:09:44 才起来，那 6 秒界面是空的。
   *
   * 现在改用 `listLocalModels`（纯扫目录 + 读 GGUF 元数据）。
   * 引擎等用户真的点了「翻译」再由 `handleRun` 里的 translate 调用
   * 触发 ensure()，符合「按需加载、不常驻占硬件」的定位。
   *
   * 模型状态（已加载/未加载）此时一律给 `unloaded`：引擎还没起，
   * 本来就没有任何模型驻留。之后由 `model://state-changed` 事件维持。
   */
  const bootstrap = useCallback(async () => {
    setEngineError("");
    try {
      const rt = await getRuntimeStatus();
      setRuntime(rt);
      if (!rt.installed) return;

      // 顺带读一下引擎状态用于底部显示端口。`service_status` 只读、
      // 不启动引擎（用的是 `current()` 而不是 `ensure()`），安全。
      getServiceStatus().then(setService).catch(() => setService(null));

      const local = await listLocalModels();
      // 保留已有状态，别一律重置成 unloaded（见 lib/modelState.ts）
      const list: RouterModel[] = mergeModelStates(
        models,
        local.map((m) => m.id),
      );
      setModels(list);
      setModel((prev) => prev || list[0]?.id || "");
    } catch (e) {
      setEngineError(String(e));
    }
    // models 只用于保留旧状态，不作为重建依赖（否则每次 setModels 都会重跑）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    bootstrap();
  }, [bootstrap, refreshSignal]);

  useTauriEvent(onTranslateProgress, setSegmentProgress);

  // 质检报告：后端发现译文有问题并自动重试过。批量任务的 requestId 为空，不在这里显示
  useTauriEvent(onQualityReport, (p) => {
    if (!p.requestId) return;
    setQuality((prev) => [...prev, p]);
  });

  // 模型状态实时同步（加载中 / 已加载 / 卸载中）
  useTauriEvent(onModelState, (p) => {
    const mapped =
      p.state === "warm" ? "loaded" : p.state === "cold" ? "unloaded" : p.state;
    setModels((prev) =>
      prev.map((m) => (m.id === p.model ? { ...m, state: mapped } : m)),
    );
    setLoad(toLoadState(p));
  });

  useEffect(() => {
    return () => {
      unlistenRef.current?.();
    };
  }, []);


  /**
   * 把 .gguf 直接拖进窗口：把它所在目录设为模型目录。
   *
   * 不做拷贝——模型动辄几个 GB，复制一份既慢又白占磁盘。
   * 拖进来的若是目录，就用目录本身。
   */
  const adoptDroppedPath = useCallback(async (paths: string[]) => {
    const hit = paths.find((p) => p.toLowerCase().endsWith(".gguf")) ?? paths[0];
    if (!hit) return;
    const dir = /\.gguf$/i.test(hit)
      ? hit.replace(/[\\/][^\\/]+$/, "")
      : hit;
    if (!dir) return;

    setEngineError("");
    try {
      await setModelsDir(dir);
      await restartService();
      await bootstrap();
      onRuntimeChanged?.();
    } catch (e) {
      setEngineError(String(e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 拖放监听：非 Tauri 环境（纯浏览器预览）没有这个 API，兜一层
  useEffect(() => {
    let un: UnlistenFn | null = null;
    let dead = false;
    try {
      getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type === "drop") {
            void adoptDroppedPath(event.payload.paths);
          }
        })
        .then((fn) => {
          // 卸载早于订阅兑现时必须补一次注销，否则监听器会越积越多
          if (dead) fn();
          else un = fn;
        });
    } catch {
      // 忽略：非 Tauri 环境没有拖放能力
    }
    return () => {
      dead = true;
      un?.();
    };
  }, [adoptDroppedPath]);

  /**
   * 弹一个 1.8 秒后自动消失的提示。
   *
   * 为什么用瞬时提示而不是常驻徽章：用户只需要**知道这一次**是缓存，
   * 知道了就够了。常驻会让标题栏一直多一块，视觉噪音大于信息量。
   */
  const showToast = useCallback((text: string) => {
    setToast(text);
    if (toastTimer.current !== null) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => {
      setToast(null);
      toastTimer.current = null;
    }, 1800);
  }, []);

  useEffect(
    () => () => {
      if (toastTimer.current !== null) window.clearTimeout(toastTimer.current);
    },
    [],
  );

  const handleRun = async (opts?: { skipCache?: boolean }) => {
    if (!model) {
      setError("请先选择模型");
      return;
    }
    if (!input.trim()) {
      setError("请输入要翻译的内容");
      return;
    }

    setError("");
    setOutput("");
    setBusy(true);
    setSegmentProgress({ done: 0, total: 0 });
    setQuality([]);
    setFromCache(false);
    // 上一次的「来自缓存」提示要立刻撤掉。
    // 否则重新翻译（这次没命中）时，旧提示会挂到新结果上，说的是反话。
    setToast(null);
    if (toastTimer.current !== null) {
      window.clearTimeout(toastTimer.current);
      toastTimer.current = null;
    }

    const id = newRequestId();
    unlistenRef.current?.();
    unlistenRef.current = await onToken((p) => {
      if (p.requestId === id) setOutput((prev) => prev + p.token);
    });

    try {
      const result = await translate({
        requestId: id,
        model,
        source,
        target,
        text: input,
        skipCache: opts?.skipCache,
      });
      // 后端返回结构：text + fromCache。
      // 早先直接拿字符串，于是无法告诉用户「这条是缓存的」——
      // 用户点「重新翻译」看到一样的译文会以为按钮坏了。
      const text = result.text ?? "";
      setOutput(text);
      setFromCache(Boolean(result.fromCache));
      if (result.fromCache) showToast("来自缓存");
    } catch (e) {
      setError(String(e));
    } finally {
      unlistenRef.current?.();
      unlistenRef.current = null;
      setBusy(false);
    }
  };

  /**
   * 能不能翻译 —— 交给 `lib/readiness` 判定，**这里不再自己算**。
   *
   * 早先这里是 `runtime?.installed && hasModels`，而引导层用的是
   * `installed && (选过目录 || 有模型)`。两套判定在「选了一个空目录」
   * 这种情况下直接分叉：引导层放行、这里报未配置。同类 bug 修过两次
   * 都没修干净，根因就是各算各的。
   */
  const readiness = computeReadiness({
    installed: Boolean(runtime?.installed),
    modelCount: models.length,
  });
  const ready = readiness.ready;


  const currentModelState =
    models.find((m) => m.id === model)?.state ?? "unloaded";

  const handleStop = async () => {
    try {
      await abort(model || undefined);
    } catch (e) {
      setError(String(e));
    }
  };

  const copyOutput = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      setError("复制失败");
    }
  };

  return (
    <div className="flex h-full flex-col gap-3 p-4">
      {/*
        「来自缓存」提示浮层。

        为什么是绝对定位的浮层而不是塞进标题栏：
        塞进 flex 布局会**挤动旁边的元素**，出现「顶一下」的跳动，
        这正是它看起来廉价的原因。浮层脱离文档流，不参与布局。
        web.dev 的 toast 规范也明确要求定位到视口而不是父容器。

        定位放在**底部居中**，不是右上角。
        右上角虽然贴近译文卡片的标题栏，但会盖住右侧的模型名 ——
        实测截图里「Hy-MT2-1.8B-Q8_0」被浮层遮掉一半。
        底部居中是译文与操作按钮之间的空白区，不会遮挡任何可交互元素。
        距底部 96px（而非 64px）：64px 时浮层恰好压在卡片下边框上，
        边框从浮层底下穿过去，看着像错位。抬高一档后完全落在卡片的空白里。
        `pointer-events-none` 保证它不抢点击（否则会挡住下面的按钮）。
      */}
      {/*
        配置引导已移到全屏覆盖层（Onboarding），
        这里只保留一条单行提示 + 重新打开的入口。
        页面本身始终是“配置好之后的样子”。
      */}
      {!ready && (
        <div
          className="flex shrink-0 items-center gap-2 rounded-lg border px-3 py-1.5 text-[12px]"
          style={{
            background: "var(--surface-muted)",
            borderColor: "var(--border)",
            color: "var(--text-muted)",
          }}
        >
          <span className="min-w-0 flex-1 truncate">
            {/*
              文案来自 `readiness.hint`，**不要写死**。
              早先这里固定写「还没配置好引擎或模型」——引擎明明配好了、
              只是缺模型时，这句话是错的，用户会以为连引擎都丢了。
              hint 会区分「还没配置推理引擎」「模型目录里还没有 .gguf
              文件」「还没配置模型目录」三种情况。
            */}
            {engineError || readiness.hint}
          </span>
          {onOpenSetup && (
            <button
              className="btn shrink-0 rounded px-1.5 text-[11px]"
              style={{ color: "var(--brand-text)" }}
              onClick={() =>
                // 缺引擎（或两样都缺）才需要全屏引导；
                // 只缺模型时走设置抽屉，不用把用户丢回"初始配置页"
                onOpenSetup(readiness.missing === "models" ? "models" : "engine")
              }
            >
              {readiness.missing === "models" ? "去添加模型" : "去配置"}
            </button>
          )}
        </div>
      )}

      {/* 工具栏：强制单行，窗口变窄时只压缩模型下拉，不换行 */}
      <div className="flex flex-nowrap items-center gap-2">
        <Select
          className="w-[112px] shrink-0"
          value={source}
          options={LANGUAGES}
          onChange={setSource}
          title="原文语言"
        />

        <span className="shrink-0 text-[12px]" style={{ color: "var(--text-muted)" }}>
          →
        </span>

        <Select
          className="w-[104px] shrink-0"
          value={target}
          options={TARGETS}
          onChange={setTarget}
          title="译文语言"
        />

        <span
          className="mx-0.5 h-5 w-px shrink-0"
          style={{ background: "var(--border)" }}
        />

        <Select
          className="min-w-0 flex-1"
          value={model}
          options={models.map((m) => ({
            value: m.id,
            label: m.id,
            hint: stateLabel(m.state),
          }))}
          onChange={setModel}
          disabled={models.length === 0}
          placeholder="未发现模型"
          title={model || "未发现模型"}
        />

        <ModelStateBadge state={currentModelState} />
      </div>

      {load && <LoadIndicator load={load} />}

      {/* 输入 / 输出 */}
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 lg:grid-cols-2">
        <section className="card flex min-h-0 flex-col overflow-hidden">
          <header className="panel-head">
            <span>原文</span>
            <span className="tabular-nums opacity-80">{input.length} 字</span>
          </header>
          <textarea
            className="min-h-0 flex-1 resize-none bg-transparent px-3.5 py-3 text-[13px] leading-relaxed outline-none placeholder:opacity-40"
            placeholder="粘贴或输入要翻译的内容…"
            value={input}
            onChange={(e) => setInput(e.target.value)}
          />
        </section>

        <section className="card relative flex min-h-0 flex-col overflow-hidden">
          {/*
            「来自缓存」浮层：贴在**译文卡片右下角**。

            为什么在卡片内而不是标题栏：塞进标题栏会挤动右侧元素，
            出现「顶一下」的跳动。绝对定位脱离文档流，不参与布局。

            为什么右下角：这是 toast 的惯例位置（Fluent 2 建议顶/底且靠右），
            目光扫完译文时自然落在这里；左下角会跟正文的起始位置承接上，
            反而像正文的一部分。四周留 12px，不贴边。

            `pointer-events-none` 保证它不抢点击（否则会挡住下面的按钮）。
          */}
          {toast && (
            <div
              className="pointer-events-none absolute bottom-3 right-3 z-20"
              role="status"
              aria-live="polite"
            >
              <span
                className="toast-pop inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-[11px]"
                style={{
                  background: "var(--surface)",
                  border: "1px solid var(--border)",
                  color: "var(--brand-text)",
                  boxShadow: "var(--shadow-3)",
                }}
              >
                {/* 小圆点：暗示「这是状态标记」而不是一段文字 */}
                <span
                  className="inline-block h-1.5 w-1.5 rounded-full"
                  style={{ background: "var(--brand)" }}
                />
                {toast}
              </span>
            </div>
          )}
          <header className="panel-head">
            <span>译文</span>
            <span className="flex min-w-0 items-center gap-2.5">
              {quality.length > 0 && (
                <QualityNote reports={quality} total={segmentProgress.total} />
              )}
              {output && !busy && (
                <button
                  className="btn rounded px-1 text-[11px] opacity-70 hover:opacity-100"
                  onClick={() => void handleRun({ skipCache: true })}
                  title={
                    fromCache
                      ? "跳过缓存，重新翻译（结果可能不同）"
                      : "重新翻译"
                  }
                >
                  重新翻译
                </button>
              )}
              {output && !busy && (
                <button
                  className="btn rounded px-1 text-[11px] opacity-70 hover:opacity-100"
                  onClick={() => copyOutput(output)}
                  title="复制译文"
                >
                  复制
                </button>
              )}
              <span className="shrink-0 tabular-nums">
                {busy
                  ? segmentProgress.total > 1
                    ? `翻译中 ${segmentProgress.done}/${segmentProgress.total}`
                    : "翻译中…"
                  : model || "—"}
              </span>
            </span>
          </header>
          <div className="min-h-0 flex-1 overflow-auto px-3.5 py-3">
            {/*
              质检没修好时的提示。做成卡片内的轻量提示条：
              有底色和边框表明"这里需要注意"，但不用红色——
              用户手上是有译文的，只是质量存疑，不该让人以为结果作废。
            */}
            {output && !busy && (
              <QualityHint
                reports={quality}
                targetLabel={
                  TARGETS.find((l) => l.value === target)?.label ?? "目标语言"
                }
                onRetry={handleRun}
                onSendToChat={
                  onSendToChat && output ? () => onSendToChat(output) : undefined
                }
              />
            )}
            {engineError && !output ? (
              <p className="m-0 text-[12px]" style={{ color: "var(--danger)" }}>
                {engineError}
              </p>
            ) : output ? (
              <p className="m-0 whitespace-pre-wrap text-[13px] leading-relaxed">
                {output}
              </p>
            ) : (
              <p className="m-0 text-[13px]" style={{ color: "var(--text-muted)" }}>
                译文会显示在这里
              </p>
            )}
          </div>
        </section>
      </div>

      {/* 操作区 */}
      <div className="flex items-center gap-3">
        {/* 开始 / 停止共用一个键，与批量页共用同一个组件，保证完全一致 */}
        <RunButton
          busy={busy}
          load={load}
          label="开始翻译"
          disabled={!busy && !ready}
          onClick={busy ? handleStop : handleRun}
        />

        {error && (
          <span className="text-[12px]" style={{ color: "var(--danger)" }}>
            {error}
          </span>
        )}
        {!error && service && (
          <span className="text-[12px]" style={{ color: "var(--text-muted)" }}>
            引擎 127.0.0.1:{service.port}
          </span>
        )}

        {output && onSendToChat && (
          <button
            className="btn h-10 rounded-lg border px-4 text-[13px]"
            style={selectStyle}
            onClick={() => onSendToChat(output)}
            title="把译文带到对话页，可以润色、改语意、增减内容"
          >
            在对话中调整
          </button>
        )}

        <button
          className="btn ml-auto h-10 rounded-lg border px-4 text-[13px]"
          style={selectStyle}
          onClick={() => {
            setInput("");
            setOutput("");
            setError("");
          }}
        >
          清空
        </button>
      </div>
    </div>
  );
}
