import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { LoadIndicator, toLoadState, type LoadState } from "../components/LoadIndicator";
import { ModelStateBadge, stateLabel } from "../components/ModelStateBadge";
import { RunButton } from "../components/RunButton";
import { Select } from "../components/Select";
import { TRANSLATE_STYLES } from "../lib/translateStyles";
import { SOURCE_LANGUAGES, TARGET_LANGUAGES } from "../lib/languages";
import {
  abort,
  discoverRuntimes,
  getRuntimeStatus,
  installRuntime,
  listRouterModels,
  newRequestId,
  onModelState,
  onQualityReport,
  onRuntimeProgress,
  onToken,
  onTranslateProgress,
  openModelsDir,
  restartService,
  setManualRuntime,
  setModelsDir,
  startService,
  stopService,
  translate,
  type DiscoveredRuntime,
  type QualityReport,
  type RouterModel,
  type RuntimeProgress,
  type RuntimeStatus,
  type ServiceStatus,
  type TranslateStyle,
} from "../lib/ipc";
import { useTauriEvent } from "../lib/useTauriEvent";

const LANGUAGES = SOURCE_LANGUAGES;
const TARGETS = TARGET_LANGUAGES;

const selectStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

function stageLabel(stage: RuntimeProgress["stage"]): string {
  switch (stage) {
    case "downloading":
      return "下载中";
    case "verifying":
      return "校验中";
    case "extracting":
      return "解压中";
    case "checking":
      return "自检中";
    default:
      return "完成";
  }
}

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
}

export function Translate({
  refreshSignal = 0,
  onSendToChat,
  onRuntimeChanged,
}: Props) {
  const [source, setSource] = useState("auto");
  const [target, setTarget] = useState("zh");
  const [style, setStyle] = useState<TranslateStyle>("general");
  const [input, setInput] = useState("");
  const [output, setOutput] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [service, setService] = useState<ServiceStatus | null>(null);
  const [models, setModels] = useState<RouterModel[]>([]);
  const [model, setModel] = useState("");
  const [progress, setProgress] = useState<RuntimeProgress | null>(null);
  const [engineError, setEngineError] = useState("");
  /** 首启时扫到的本机引擎：有就列出来点一下即可，不必去翻文件对话框。 */
  const [discovered, setDiscovered] = useState<DiscoveredRuntime[]>([]);
  /** 自动扫描只跑一次，避免每次 refresh 都扫盘。 */
  const scannedRef = useRef(false);
  /** 正在切换引擎，按钮防重入。 */
  const [engineBusy, setEngineBusy] = useState(false);
  const [load, setLoad] = useState<LoadState | null>(null);
  const [segmentProgress, setSegmentProgress] = useState({
    done: 0,
    total: 0,
  });
  /** 本次翻译里被质检挑出问题、已自动重试的片段 */
  const [quality, setQuality] = useState<QualityReport[]>([]);

  const unlistenRef = useRef<UnlistenFn | null>(null);

  /** 走完整链路：确认运行时 → 拉起引擎 → 拉模型列表 */
  const bootstrap = useCallback(async () => {
    setEngineError("");
    try {
      const rt = await getRuntimeStatus();
      setRuntime(rt);
      if (!rt.installed) return;

      await startService().then(setService);
      const list = await listRouterModels();
      setModels(list);
      setModel((prev) => prev || list[0]?.id || "");
    } catch (e) {
      setEngineError(String(e));
    }
  }, []);

  useEffect(() => {
    bootstrap();
  }, [bootstrap, refreshSignal]);

  useTauriEvent(onRuntimeProgress, setProgress);

  // 长文分段翻译的进度
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

  const handleInstall = async (backend: string) => {
    setEngineError("");
    try {
      const rt = await installRuntime(backend);
      setRuntime(rt);
      setProgress(null);
      await bootstrap();
      // 批量页/对话页是常驻挂载的，不会自己重新拉模型；装完运行时统一通知一次
      onRuntimeChanged?.();
    } catch (e) {
      setProgress(null);
      setEngineError(String(e));
    }
  };

  const handleManual = async (path: string) => {
    setEngineError("");
    setEngineBusy(true);
    try {
      setRuntime(await setManualRuntime(path));
      await restartService();
      await bootstrap();
      onRuntimeChanged?.();
    } catch (e) {
      setEngineError(String(e));
    } finally {
      setEngineBusy(false);
    }
  };

  /**
   * 用系统文件对话框挑一个 llama-server。
   *
   * 原来这里是 `window.prompt` 手打路径。Tauri 只 polyfill 了 alert/confirm，
   * prompt 走 WebView2 原生实现：不能预设目录、不能过滤扩展名、不能校验，
   * 而且默认值只能是写死的一串路径——对别人机器根本不成立。
   */
  const pickEngine = async () => {
    setEngineError("");
    try {
      const picked = await openDialog({
        multiple: false,
        directory: false,
        title: "选择 llama-server 可执行文件",
        filters: [{ name: "可执行文件", extensions: ["exe"] }],
      });
      if (typeof picked !== "string") return;
      await handleManual(picked);
    } catch (e) {
      setEngineError(String(e));
    }
  };

  /** 选一个已有的模型目录（同样替掉原来的手打路径）。 */
  const pickModelsDir = async () => {
    setEngineError("");
    try {
      const picked = await openDialog({
        multiple: false,
        directory: true,
        title: "选择存放 .gguf 的模型目录",
      });
      if (typeof picked !== "string") return;
      await setModelsDir(picked);
      await stopService();
      await bootstrap();
      onRuntimeChanged?.();
    } catch (e) {
      setEngineError(String(e));
    }
  };

  /**
   * 首启自动扫一遍磁盘找现成引擎。
   *
   * 多数人机器上本就有一份 llama.cpp（自己下的 CUDA 版之类），
   * 没必要让人先去设置里点「扫描」——扫到了直接接上才是开箱即用。
   * 只有唯一候选才自动采用；有多个则列出来让用户自己挑。
   */
  const autoAdoptEngine = useCallback(async () => {
    if (scannedRef.current) return;
    scannedRef.current = true;
    try {
      const found = await discoverRuntimes();
      setDiscovered(found);
      if (found.length === 1) await handleManual(found[0].path);
    } catch {
      // 扫描失败不拦人，手动入口还在
    }
  }, []);

  // 只在「还没引擎」时自动扫一次；装好之后就没必要再扫盘了
  useEffect(() => {
    if (runtime && !runtime.installed) void autoAdoptEngine();
  }, [runtime, autoAdoptEngine]);

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

  const handleRun = async () => {
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

    const id = newRequestId();
    unlistenRef.current?.();
    unlistenRef.current = await onToken((p) => {
      if (p.requestId === id) setOutput((prev) => prev + p.token);
    });

    try {
      const text = await translate({
        requestId: id,
        model,
        source,
        target,
        style,
        text: input,
      });
      setOutput(text);
    } catch (e) {
      setError(String(e));
    } finally {
      unlistenRef.current?.();
      unlistenRef.current = null;
      setBusy(false);
    }
  };

  /** 模型列表非空即算这一步完成（引擎没装时不算，因为拉不到列表）。 */
  const hasModels = models.length > 0;
  const ready = runtime?.installed && hasModels;

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
        首启清单：把「装引擎」和「放模型」两个前置条件放在一张卡上。
        原来这是两块独立卡片，装完引擎才浮现第二块，用户得先后撞两次墙；
        合并后能一眼看出还差几步。
      */}
      {(!runtime?.installed || models.length === 0) && (
        <section className="card p-3.5">
          <div className="mb-2 flex items-center gap-2">
            <span className="text-[13px] font-medium">开始使用</span>
            <span
              className="rounded-full px-2 py-0.5 text-[11px] tabular-nums"
              style={{ background: "var(--brand-soft)", color: "var(--brand-text)" }}
            >
              {ready ? 2 : runtime?.installed || models.length > 0 ? 1 : 0} / 2
            </span>
            <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
              装好引擎、放进模型就能翻译
            </span>
          </div>

          {/* ① 推理引擎 */}
          <div className="flex items-start gap-2.5 py-1.5">
            <span
              className="mt-0.5 w-3.5 shrink-0 text-center text-[12px]"
              style={{
                color: runtime?.installed ? "var(--brand-text)" : "var(--text-muted)",
              }}
            >
              {runtime?.installed ? "✓" : "○"}
            </span>
            <div className="min-w-0 flex-1">
              <div className="text-[12px]">推理引擎</div>

              {progress ? (
                <div className="mt-1">
                  <div
                    className="mb-1 flex justify-between text-[11px]"
                    style={{ color: "var(--text-muted)" }}
                  >
                    <span>{stageLabel(progress.stage)}</span>
                    <span className="tabular-nums">
                      {progress.stage === "downloading"
                        ? `${progress.percent.toFixed(1)}% · ${(
                            progress.downloaded / 1048576
                          ).toFixed(1)} / ${(progress.total / 1048576).toFixed(
                            1,
                          )} MB`
                        : ""}
                    </span>
                  </div>
                  <div
                    className="h-1.5 overflow-hidden rounded-full"
                    style={{ background: "var(--surface-muted)" }}
                  >
                    <div
                      className="h-full rounded-full transition-[width] duration-200"
                      style={{
                        width: `${progress.percent}%`,
                        background: "var(--brand)",
                      }}
                    />
                  </div>
                </div>
              ) : runtime?.installed ? (
                <div
                  className="mt-0.5 truncate text-[11px]"
                  style={{ color: "var(--text-muted)" }}
                  title={runtime.serverPath ?? ""}
                >
                  {runtime.version} · {runtime.backend}
                </div>
              ) : (
                <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
                  <button
                    className="btn h-7 rounded-md px-2.5 text-[11px] font-medium text-white"
                    style={{ background: "var(--brand)" }}
                    onClick={() => handleInstall("vulkan")}
                    title="下载并解压到应用私有目录，约 31 MB"
                  >
                    下载运行时（约 31 MB）
                  </button>
                  <button
                    className="btn h-7 rounded-md border px-2 text-[11px]"
                    style={{ ...selectStyle }}
                    onClick={pickEngine}
                    disabled={engineBusy}
                  >
                    {engineBusy ? "切换中…" : "选择文件…"}
                  </button>

                  {/*
                    只写后端会撞车——同一台机器常有两个 CUDA 构建，
                    光看「cuda」分不清哪个是哪个。把 build 号提出来才选得出。
                    完整版本行太长，放进 tooltip。
                  */}
                  {discovered.map((d) => {
                    const build = /build\s+(\d+)/i.exec(d.version)?.[1];
                    return (
                      <button
                        key={d.path}
                        className="btn h-7 max-w-[240px] rounded-md border px-2 text-[11px]"
                        style={{ ...selectStyle }}
                        onClick={() => handleManual(d.path)}
                        disabled={engineBusy}
                        title={`${d.path}\n${d.version}`}
                      >
                        <span className="block truncate">
                          用本机 {d.backend}
                          {build ? ` b${build}` : ""}
                        </span>
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          </div>

          {/* ② 模型 */}
          <div className="flex items-start gap-2.5 py-1.5">
            <span
              className="mt-0.5 w-3.5 shrink-0 text-center text-[12px]"
              style={{
                color: hasModels ? "var(--brand-text)" : "var(--text-muted)",
              }}
            >
              {hasModels ? "✓" : "○"}
            </span>
            <div className="min-w-0 flex-1">
              <div className="text-[12px]">模型</div>

              {!runtime?.installed ? (
                <div
                  className="mt-0.5 text-[11px]"
                  style={{ color: "var(--text-muted)" }}
                >
                  装好引擎后再放模型
                </div>
              ) : hasModels ? (
                <div
                  className="mt-0.5 truncate text-[11px]"
                  style={{ color: "var(--text-muted)" }}
                  title={runtime.modelsDir}
                >
                  {models.length} 个可用 · {runtime.modelsDir}
                </div>
              ) : (
                <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
                  <button
                    className="btn h-7 rounded-md border px-2 text-[11px]"
                    style={{ ...selectStyle }}
                    onClick={async () => {
                      await openModelsDir();
                      setEngineError("");
                      bootstrap();
                    }}
                  >
                    打开模型目录
                  </button>
                  <button
                    className="btn h-7 rounded-md border px-2 text-[11px]"
                    style={{ ...selectStyle }}
                    onClick={pickModelsDir}
                  >
                    选择已有目录…
                  </button>
                  <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
                    也可以直接把 .gguf 拖进窗口
                  </span>
                </div>
              )}
            </div>
          </div>

          {!progress && (
            <p
              className="m-0 mt-1.5 text-[11px]"
              style={{ color: "var(--text-muted)" }}
            >
              Vulkan 是通用后端，首次推理约 3 秒着色器编译；已有 CUDA 版的话，
              用「选择文件…」或上面的本机候选直接指过去，不必重新下载。
            </p>
          )}

          {engineError && (
            <p className="m-0 mt-1.5 text-[12px]" style={{ color: "var(--danger)" }}>
              {engineError}
            </p>
          )}
        </section>
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

        <Select
          className="w-[92px] shrink-0"
          value={style}
          options={TRANSLATE_STYLES}
          onChange={setStyle}
          title="翻译风格"
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

        <section className="card flex min-h-0 flex-col overflow-hidden">
          <header className="panel-head">
            <span>译文</span>
            <span className="flex min-w-0 items-center gap-2.5">
              {quality.length > 0 && (
                <QualityNote reports={quality} total={segmentProgress.total} />
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
