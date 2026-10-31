import { useCallback, useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  discoverRuntimes,
  getRuntimeStatus,
  installRuntime,
  listRouterModels,
  onRuntimeProgress,
  restartService,
  setManualRuntime,
  setModelsDir,
  type DiscoveredRuntime,
  type RouterModel,
  type RuntimeProgress,
  type RuntimeStatus,
} from "../lib/ipc";
import { useTauriEvent } from "../lib/useTauriEvent";

/**
 * 首次启动引导。
 *
 * 为什么是**全屏覆盖层**而不是塞在某个页面里：翻译页、批量页、对话页
 * 都该是"配置好之后的样子"。把清单卡嵌在翻译页顶部，等于让基础功能
 * 长期挂着一块与它无关的配置面板——用户没说，但看得出来很别扭。
 *
 * 覆盖层盖在最上层，配好就整个消失，三个页面从来不受污染。
 * 设置抽屉里保留入口，随时能重新打开。
 *
 * 为什么不做分步向导：要配的只有两件事（引擎、模型），一次看完更省事，
 * 分页反而让人多点几次。三个入口（扫描/选择文件/下载）也同时摆着，
 * 不强制顺序——手里已经有引擎的人可以直接点候选。
 */
interface Props {
  /** 关闭后调用（已配置完成或用户主动跳过）。 */
  onDone: () => void;
  /** 配置有变动，通知外层刷新模型列表。 */
  onChanged: () => void;
}

const btnStyle = {
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

export function Onboarding({ onDone, onChanged }: Props) {
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [models, setModels] = useState<RouterModel[]>([]);
  const [discovered, setDiscovered] = useState<DiscoveredRuntime[]>([]);
  const [progress, setProgress] = useState<RuntimeProgress | null>(null);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  /** 已经扫过一次就不再重复扫盘。 */
  const scannedRef = useRef(false);

  const refresh = useCallback(async () => {
    try {
      const rt = await getRuntimeStatus();
      setRuntime(rt);
      if (!rt.installed) {
        setModels([]);
        return;
      }
      try {
        setModels(await listRouterModels());
      } catch {
        setModels([]);
      }
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useTauriEvent(onRuntimeProgress, setProgress);

  /** 扫一遍磁盘，列出可以直接用的引擎。 */
  const scan = useCallback(async () => {
    if (scannedRef.current) return;
    scannedRef.current = true;
    try {
      setDiscovered(await discoverRuntimes());
    } catch {
      // 扫描失败不拦人，手动入口还在
    }
  }, []);

  useEffect(() => {
    void scan();
  }, [scan]);

  const adoptEngine = async (path: string) => {
    setError("");
    setBusy("engine");
    try {
      setRuntime(await setManualRuntime(path));
      await restartService();
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const pickEngine = async () => {
    setError("");
    try {
      const picked = await openDialog({
        multiple: false,
        directory: false,
        title: "选择 llama-server 可执行文件",
        filters: [{ name: "可执行文件", extensions: ["exe"] }],
      });
      if (typeof picked !== "string") return;
      await adoptEngine(picked);
    } catch (e) {
      setError(String(e));
    }
  };

  const download = async () => {
    setError("");
    setBusy("download");
    try {
      setRuntime(await installRuntime("vulkan"));
      setProgress(null);
      await refresh();
      onChanged();
    } catch (e) {
      setProgress(null);
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const pickDir = async () => {
    setError("");
    try {
      const picked = await openDialog({
        multiple: false,
        directory: true,
        title: "选择存放 .gguf 的模型目录",
      });
      if (typeof picked !== "string") return;
      await setModelsDir(picked);
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const engineReady = Boolean(runtime?.installed);
  const modelReady = models.length > 0;
  const allReady = engineReady && modelReady;

  return (
    <div
      className="fixed inset-0 z-50 overflow-y-auto"
      style={{ background: "var(--canvas)" }}
    >
      {/*
        居中用 mx-auto 而不是 flex justify-center：
        flex 居中在内容高于视口时会顶掉上边距、且横向滚动条出现时
        会把整块推偏。auto 外边距 + 自身限宽在任何窗口尺寸下都居中。
      */}
      <div className="mx-auto w-full max-w-[560px] px-6 py-10">
        {/* 标题 */}
        <div className="mb-7 text-center">
          <div className="text-[26px] font-semibold tracking-tight">
            Xieyi
            <span
              className="ml-2 text-[15px] font-normal"
              style={{ color: "var(--text-muted)" }}
            >
              写意
            </span>
          </div>
          <p
            className="m-0 mt-2 text-[13px]"
            style={{ color: "var(--text-muted)" }}
          >
            本地翻译，内容不出本机
          </p>
        </div>

        {/* ① 推理引擎 */}
        <section className="card mb-3 p-4">
          <div className="mb-3 flex items-center gap-2">
            <span
              className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-[11px]"
              style={{
                background: engineReady
                  ? "var(--brand-soft)"
                  : "var(--surface-muted)",
                color: engineReady ? "var(--brand-text)" : "var(--text-muted)",
              }}
            >
              {engineReady ? "✓" : "1"}
            </span>
            <span className="text-[13px] font-medium">推理引擎</span>
            {engineReady && (
              <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
                {runtime?.version} · {runtime?.backend}
              </span>
            )}
          </div>

          {progress ? (
            <div>
              <div
                className="mb-1.5 flex justify-between text-[11px]"
                style={{ color: "var(--text-muted)" }}
              >
                <span>{stageLabel(progress.stage)}</span>
                <span className="tabular-nums">
                  {progress.stage === "downloading"
                    ? `${progress.percent.toFixed(1)}% · ${(
                        progress.downloaded / 1048576
                      ).toFixed(1)} / ${(progress.total / 1048576).toFixed(1)} MB`
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
          ) : (
            <>
              {/*
                候选列表**始终显示**，选中的那个带高亮。
                早先选中后整列消失，点错了就回不去——只能去文件对话框重新找路径。
                现在随时能在候选之间来回换。
              */}
              {discovered.length > 0 && (
                <ul className="m-0 mb-2.5 flex list-none flex-col gap-1.5 p-0">
                  {discovered.map((d) => {
                    const current = d.path === runtime?.serverPath;
                    return (
                      <li key={d.path}>
                        <button
                          className="engine-cand w-full text-left"
                          data-current={current}
                          onClick={() => adoptEngine(d.path)}
                          disabled={busy !== "" || current}
                          title={d.path}
                        >
                          <span className="flex items-center gap-2">
                            <span className="min-w-0 flex-1 truncate text-[12px]">
                              {d.label}
                            </span>
                            <span
                              className="shrink-0 rounded px-1.5 py-0.5 text-[10px]"
                              style={{
                                background: "var(--brand-soft)",
                                color: "var(--brand-text)",
                              }}
                            >
                              {d.backend}
                            </span>
                          </span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              )}

              <div className="flex flex-wrap items-center gap-1.5">
                <button
                  className="btn h-8 rounded-lg border px-3 text-[12px]"
                  style={btnStyle}
                  onClick={pickEngine}
                  disabled={busy !== ""}
                >
                  指定其他引擎…
                </button>
                <button
                  className="btn h-8 rounded-lg border px-3 text-[12px]"
                  style={btnStyle}
                  onClick={download}
                  disabled={busy !== ""}
                  title="从 GitHub 下载 llama.cpp 推理引擎（Vulkan 版，约 31 MB）"
                >
                  下载推理引擎…
                </button>
              </div>

              {/* 说清「下载」到底下的是什么，第一次用的人不会一头雾水 */}
              <p
                className="m-0 mt-2 text-[11px]"
                style={{ color: "var(--text-muted)" }}
              >
                下载的是 llama.cpp 推理引擎（Vulkan 版，约 31 MB）。
                如果本机已有 llama-server，直接用上面的候选即可。
              </p>

              {/* 当前生效的引擎路径：让用户确认选中的到底是哪个 */}
              {engineReady && (
                <p
                  className="m-0 mt-2 truncate text-[11px]"
                  style={{ color: "var(--text-muted)" }}
                  title={runtime?.serverPath ?? ""}
                >
                  当前：{runtime?.serverPath}
                </p>
              )}
            </>
          )}
        </section>

        {/* ② 模型 */}
        <section className="card mb-3 p-4">
          <div className="mb-3 flex items-center gap-2">
            <span
              className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-[11px]"
              style={{
                background: modelReady
                  ? "var(--brand-soft)"
                  : "var(--surface-muted)",
                color: modelReady ? "var(--brand-text)" : "var(--text-muted)",
              }}
            >
              {modelReady ? "✓" : "2"}
            </span>
            <span className="text-[13px] font-medium">模型</span>
            {modelReady && (
              <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
                已发现 {models.length} 个
              </span>
            )}
          </div>

          {!engineReady ? (
            <p
              className="m-0 text-[12px]"
              style={{ color: "var(--text-muted)" }}
            >
              请先完成推理引擎配置
            </p>
          ) : (
            <>
              <p
                className="m-0 mb-2.5 truncate text-[11px]"
                style={{ color: "var(--text-muted)" }}
                title={runtime?.modelsDir ?? ""}
              >
                {runtime?.modelsDir || "（未设置）"}
              </p>
              <div className="flex flex-wrap items-center gap-1.5">
                <button
                  className="btn h-8 rounded-lg border px-3 text-[12px]"
                  style={btnStyle}
                  onClick={pickDir}
                >
                  选择模型目录…
                </button>
              </div>
              <p
                className="m-0 mt-2 text-[11px]"
                style={{ color: "var(--text-muted)" }}
              >
                支持将 .gguf 文件直接拖入窗口
              </p>
            </>
          )}
        </section>

        {error && (
          <p className="m-0 mb-3 text-[12px]" style={{ color: "var(--danger)" }}>
            {error}
          </p>
        )}

        {/*
          未配置完成时按钮是**描边样式**，不是灰底：
          灰底灰字读起来像"禁用"，但那其实是可点的次要操作。
          描边既压住了视觉层级，又明确表达"这个能按"。
        */}
        <div className="flex items-center gap-2">
          <button
            className="btn h-10 flex-1 rounded-lg px-4 text-[13px] font-medium"
            style={
              allReady
                ? { background: "var(--brand)", color: "#fff", border: "none" }
                : {
                    background: "var(--surface)",
                    color: "var(--text)",
                    border: "1px solid var(--border-strong)",
                  }
            }
            onClick={onDone}
            title={allReady ? "进入主界面" : "配置会被保留，可稍后继续"}
          >
            {allReady ? "开始使用" : "稍后配置"}
          </button>
        </div>

        <p
          className="m-0 mt-3 text-center text-[11px]"
          style={{ color: "var(--text-muted)" }}
        >
          以上设置可随时通过右上角「设置」调整
        </p>
      </div>
    </div>
  );
}
