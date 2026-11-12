import { useCallback, useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  discoverRuntimes,
  getAppInfo,
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
import { computeReadiness } from "../lib/readiness";

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

/// 候选列表超过这个数量才限高滚动。
///
/// 参考 Semrush Intergalactic 的 Select 规范：菜单超过 7 项才需要
/// 限高 + 滚动，否则列表本身就能一眼看完，滚动条是多余噪音。
const MAX_VISIBLE_ENGINES = 7;

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

/**
 * 把路径压成「开头…结尾」的形式，保住盘符和目录名。
 *
 * 直接用 CSS `truncate` 是**截尾**的，于是
 *   `C:\Users\wbai\Desktop\scratch-models`
 * 会显示成 `C:\Users\wbai\Desktop\sc`——恰恰把用户最需要辨认的
 * 目录名切没了。而 `direction: rtl` 那个经典技巧依赖浏览器把反斜杠
 * 当路径分隔符，Windows 路径上它并不认，不同 WebView 版本表现不一。
 *
 * 所以这里自己算，行为可预测：
 *   - 够短就原样返回
 *   - 太长就保留前 `head` 个字符 + `…` + 后 `tail` 个字符
 *     结尾留得比开头多：目录名比盘符更有辨识度
 *
 * 长度按**显示宽度**粗算：中日韩字符算 2，其余算 1。
 * 路径里通常没有中文，但用户名/目录名可能是中文，不区分会明显偏窄。
 */
export function shortenPath(path: string, max = 44): string {
  if (!path) return "";
  const cellWidth = (c: string) => (c.codePointAt(0)! > 0x2e80 ? 2 : 1);
  const width = (s: string) =>
    [...s].reduce((w, c) => w + cellWidth(c), 0);
  if (width(path) <= max) return path;

  const chars = [...path];

  // 结尾留得比开头多：目录名比盘符更有辨识度。
  // 目标宽度按 6:4 分配，然后**按实际显示宽度**从两端往中间收——
  // 不能按字符个数切，否则中文路径（每字占 2 格）会明显超宽。
  const tailBudget = Math.floor(max * 0.6);
  const headBudget = max - tailBudget - 1; // 减去省略号本身

  let headW = 0;
  let headEnd = 0;
  while (headEnd < chars.length && headW + cellWidth(chars[headEnd]) <= headBudget) {
    headW += cellWidth(chars[headEnd]);
    headEnd += 1;
  }

  let tailW = 0;
  let tailStart = chars.length;
  while (
    tailStart > headEnd &&
    tailW + cellWidth(chars[tailStart - 1]) <= tailBudget
  ) {
    tailStart -= 1;
    tailW += cellWidth(chars[tailStart]);
  }

  return `${chars.slice(0, headEnd).join("")}…${chars.slice(tailStart).join("")}`;
}

export function Onboarding({ onDone, onChanged }: Props) {
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [models, setModels] = useState<RouterModel[]>([]);
  const [discovered, setDiscovered] = useState<DiscoveredRuntime[]>([]);
  /// 用户是否显式设过模型目录（来自 AppInfo，不是 RuntimeStatus）
  const [dirConfigured, setDirConfigured] = useState(false);
  /**
   * 模型目录的显示值。
   *
   * **单独存一份，不再直接读 `runtime.modelsDir`。**
   *
   * 原因：用户选完目录后，界面得**立刻**反映新路径。如果只依赖
   * `runtime`，就得等 refresh() 把 getRuntimeStatus 拉回来——中间
   * 那段（尤其重启引擎要 1-3 秒）路径框还显示旧值，用户会以为
   * 没选成功，反复再点。
   *
   * 所以这里做「乐观更新」：选完立刻写本地状态，refresh() 回来后
   * 以后端值为准再校正一次。
   */
  const [modelsDirShown, setModelsDirShown] = useState("");
  /// 模型扫描中。
  ///
  /// 扫描要读每个 GGUF 的元数据，模型多的用户会等几秒。
  /// 不给提示时，用户只看到「开始使用」灰着，不知道在等什么。
  const [scanning, setScanning] = useState(false);
  const [progress, setProgress] = useState<RuntimeProgress | null>(null);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  /** 已经扫过一次就不再重复扫盘。 */
  const scannedRef = useRef(false);
  /**
   * 组件是否还挂着。
   *
   * `refresh()` 会在选目录 / 下载引擎 / 切换引擎后反复调用，
   * 而用户随时可能点「开始使用」把这一层卸掉——那时异步回来的
   * `setState` 会打到已卸载的组件上，React 直接报错。
   *
   * ⚠️ **挂载时必须把它设回 true，不能只写卸载时的 false。**
   *
   * `useRef(true)` 的初始值只在**首次**渲染时生效。而 StrictMode 会
   * 故意「挂载 → 卸载 → 再挂载」一遍来暴露副作用问题：
   *
   *   首次挂载  → ref = true
   *   模拟卸载  → 清理函数把 ref 置 false（且**不会**被重置）
   *   再次挂载  → ref 仍是 false
   *
   * 于是 `refresh()` 里每个 `if (!aliveRef.current) return` 全部提前
   * 返回，状态永远不更新——表现就是「选了模型目录，路径却不刷新」。
   * 这个回归就是漏了下面这行 `aliveRef.current = true` 造成的。
   */
  const aliveRef = useRef(true);

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
    };
  }, []);

  const refresh = useCallback(async () => {
    try {
      // 用户是否真的选过目录，只有 AppInfo 知道：
      // RuntimeStatus.modelsDir 总有默认值，不能用来判断。
      //
      // **必须 await**：早先这里是 `.then(...)` 甩出去的，于是
      // `dirConfigured` 可能晚于下面的 `setRuntime` 落地，界面先按
      // 旧值渲染一帧再跳变。卸载保护见下面的 `aliveRef`。
      try {
        const info = await getAppInfo();
        if (!aliveRef.current) return;
        setDirConfigured(info.modelsDirConfigured);
      } catch {
        if (!aliveRef.current) return;
        setDirConfigured(false);
      }

      const rt = await getRuntimeStatus();
      if (!aliveRef.current) return;
      setRuntime(rt);
      // 以后端值为准校正显示（覆盖乐观更新，也覆盖首次加载）
      if (rt.modelsDir) setModelsDirShown(rt.modelsDir);
      if (!rt.installed) {
        setModels([]);
        return;
      }
      setScanning(true);
      try {
        const list = await listRouterModels();
        if (aliveRef.current) setModels(list);
      } catch {
        if (aliveRef.current) setModels([]);
      } finally {
        if (aliveRef.current) setScanning(false);
      }
    } catch (e) {
      if (aliveRef.current) setError(String(e));
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
      // 先就地更新显示，**不要等下面那串异步往返**。
      // 重启引擎实测要十几秒（日志：01:30:27 → 01:30:40），
      // 中间路径框还显示旧值的话，用户会以为没选上。
      setModelsDirShown(picked);
      setDirConfigured(true);

      // 重启引擎期间必须有反馈：这是整个引导流程里唯一一段
      // 真正会让用户干等十几秒的操作。没有提示时表现是
      // 「点了按钮，什么都没发生」，用户会反复再点，叠出多个重启。
      setBusy("dir");
      await setModelsDir(picked);
      // **改目录必须重启引擎。**
      //
      // `--models-dir` 是 llama-server 的**启动参数**，进程活着的期间
      // 始终指向启动那一刻的目录。只写 config.json 的话：
      //   1. 配置确实落了盘，设置里也能看到新路径
      //   2. 但引擎还在扫旧目录 → listRouterModels 返回旧结果（多半是空）
      //   3. 于是 modelReady 恒为 false，界面永远停在「稍后配置」
      //
      // 设置抽屉里的同一个操作（ModelDrawer 的 changeDir）一直是带
      // restart 的，只有这里漏了——两处实现不一致，这类 bug 就藏在这种
      // 地方。改完要保持两边一致。
      await restartService();
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  /**
   * 判定走 `lib/readiness`，不在这里自己算。
   *
   * 这里关心的是**能不能进主界面**（`canEnter` = 引擎就绪即可），
   * 不是**能不能翻译**（`ready` = 还得有模型）。
   *
   * 这两件事早先被混成一个，来回错了两次：
   *   1. 先是用 `Boolean(runtime?.modelsDir)` 判「选过目录」——恒为 true
   *   2. 改成只看「有模型」——把「想先进去看看」的用户挡在门外，形成
   *      「不放模型就进不去、进不去就看不到模型目录在哪」的死循环
   *
   * 现在的分工：进门只看引擎；缺模型在页面里如实提示，而不是拦在门外。
   */
  const readiness = computeReadiness({
    installed: Boolean(runtime?.installed),
    modelCount: models.length,
    scanning,
    dirConfigured,
  });
  // 两个分步徽章各自要显示「✓ / 数字」，取同一份判定里的结果，
  // 避免这里和判定又对不上。
  const engineReady = readiness.engineReady;
  const modelReady = readiness.modelReady;
  /** 可以进主界面（引擎就绪即可，模型可以稍后再配） */
  const allReady = readiness.canEnter;

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
      <div
        className="onboarding-pad mx-auto flex min-h-full w-full max-w-[560px] flex-col justify-center px-6 py-6"
      >
        {/* 标题 */}
        <div className="onboarding-head mb-5 text-center">
          <img
            src="/swallow.png"
            alt=""
            width={64}
            height={64}
            className="onboarding-logo mx-auto mb-2.5"
            aria-hidden
          />
          <div className="onboarding-title text-[24px] font-semibold tracking-tight">
            Xieyi
            <span
              className="ml-2 text-[15px] font-normal"
              style={{ color: "var(--text-muted)" }}
            >
              写意
            </span>
          </div>
          <p
            className="onboarding-subtitle m-0 mt-2 text-[13px]"
            style={{ color: "var(--text-muted)" }}
          >
            本地翻译，内容不出本机
          </p>
        </div>

        {/*
          内容区可滚动，但**标题与主按钮固定**。
          整页滚动时用户会不知道「开始使用」在哪——
          小窗口下那个按钮可能在视口外。
        */}
        <div className="min-h-0 flex-1 overflow-y-auto">
        {/* ① 推理引擎 */}
        <section className="onboarding-card card mb-3 p-4">
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
                {/*
                  手动指定的引擎，version / backend 都是字面量 "manual"
                  （见 runtime.rs 的 write_manual_info），直接拼出来是
                  「manual · manual」，对用户毫无信息量。
                  这种情况改说「手动指定」，才是在讲人话。
                */}
                {runtime?.backend === "manual"
                  ? "手动指定"
                  : `${runtime?.version} · ${runtime?.backend}`}
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
                <ul
                  className="m-0 mb-2.5 flex list-none flex-col gap-1.5 p-0"
                  style={
                    // 超过 7 项才限高滚动（业界惯例）。
                    // 多数用户只有 1-3 个引擎，滚动条纯属噪音。
                    discovered.length > MAX_VISIBLE_ENGINES
                      ? { maxHeight: 7 * 44, overflowY: "auto" }
                      : undefined
                  }
                >
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
        <section className="onboarding-card card mb-3 p-4">
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
            {/*
              标题右侧固定这一个位置，按状态互斥显示一句短提示。

              三种状态共用同一处，**不各占一行**：
                - 扫描中   → 「正在扫描…」
                - 已扫到   → 「已发现 N 个」
                - 目录空   → 「没有 .gguf 文件」（danger 色）

              早先「没找到模型」是另起一行放在卡片底部的，会把卡片
              硬顶高一行；而同一件事在标题旁已经有位置了，等于同一个
              信息占了两处、其中一处还把布局撑变形。合并到一处后，
              卡片高度稳定，用户视线也不用跳。

              为什么用 `ml-auto` 靠右而不是紧贴左边：这行提示是**状态**，
              不是标题的一部分。靠右后与其它卡片的「当前：…」形成同一条
              视觉轴，扫视时不会被误读成第二个标题。
            */}
            <span className="ml-auto shrink-0 text-[11px]">
              {modelReady ? (
                <span style={{ color: "var(--brand-text)" }}>
                  已发现 {models.length} 个
                </span>
              ) : engineReady && dirConfigured && !scanning ? (
                <span style={{ color: "var(--danger)" }}>没有 .gguf 文件</span>
              ) : null}
            </span>
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
              {/*
                路径只做展示，**不做成按钮**。

                曾经把它做成整行可点（点击也打开目录选择），想法是
                "扩大点击区域"。但下面已经有一个明确的「选择模型目录…」
                按钮做同一件事，两个入口指向同一个动作只会让人犹豫
                "这两个是不是不一样"。

                展示态的要点：
                - 「当前目录」标签先把这行的语义说清楚
                - 值用等宽字体，反斜杠与字母宽度一致
                - 长路径中间省略，保住盘符和目录名（见 shortenPath）
                - 悬停 title 看完整路径
              */}
              <div
                className="mb-2.5 flex items-center gap-2 rounded-lg border px-2.5 py-1.5"
                style={btnStyle}
                title={modelsDirShown || "尚未设置模型目录"}
              >
                <span
                  className="shrink-0 text-[11px]"
                  style={{ color: "var(--text-muted)" }}
                >
                  当前目录
                </span>
                <span
                  className="min-w-0 flex-1 text-[11px]"
                  style={{
                    fontFamily:
                      "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
                    color: modelsDirShown ? "var(--text)" : "var(--text-muted)",
                    // 路径用**中间省略**：保住盘符/开头和目录名/结尾。
                    //
                    // 为什么不用 CSS 的 `text-overflow: ellipsis`：
                    // `truncate` 是截尾，于是
                    //   C:\Users\wbai\Desktop\scratch-models
                    // 会变成 `C:\Users\wbai\Desktop\sc` —— 恰恰把用户
                    // 最需要辨认的那一段（目录名）切没了。
                    //
                    // 也不想用 `direction: rtl` 那个经典技巧：Windows 路径
                    // 用反斜杠，浏览器不认它是路径分隔符，省略号出现在哪
                    // 一侧在不同 WebView 版本上表现不一致，还要配 bdi 兜底。
                    // 这里直接算好中间省略，行为完全可预测。
                    overflow: "hidden",
                    whiteSpace: "nowrap",
                  }}
                >
                  {shortenPath(modelsDirShown) || "（未设置）"}
                </span>
              </div>
              <div className="flex flex-wrap items-center gap-1.5">
                <button
                  className="btn h-8 rounded-lg border px-3 text-[12px]"
                  style={btnStyle}
                  onClick={pickDir}
                  disabled={busy !== ""}
                >
                  {busy === "dir"
                    ? "正在重启引擎…"
                    : modelsDirShown
                      ? "更换目录…"
                      : "选择模型目录…"}
                </button>
                {/*
                  重启引擎是整段引导里唯一会让用户干等十几秒的操作，
                  必须把那十几秒说清楚，否则表现就是「点了没反应」。
                  这里单独一行小字，不占标题右侧那个状态位——
                  那个位置说的是「扫到几个模型」，不是「正在忙什么」。
                */}
                {busy === "dir" && (
                  <span
                    className="text-[11px]"
                    style={{ color: "var(--text-muted)" }}
                  >
                    正在让引擎切到新目录…
                  </span>
                )}
              </div>
              <p
                className="m-0 mt-2 text-[11px]"
                style={{ color: "var(--text-muted)" }}
              >
                支持将 .gguf 文件直接拖入窗口
              </p>
              {/* 「没找到 .gguf」的提示已合并到标题右侧这一处，
                  不再在这里另起一行把卡片撑高。 */}
            </>
          )}
        </section>
        </div>

        {/*
          错误提示做成不占布局的浮条。
          原来它插在按钮上方，一出现就把「开始使用」往下推，
          整个引导层的视觉重心跟着跳。
        */}
        {error && (
          <div
            className="pointer-events-none fixed inset-x-0 bottom-6 z-50 flex justify-center px-4"
            role="alert"
          >
            <p
              className="m-0 rounded-lg border px-3 py-2 text-[12px]"
              style={{
                background: "var(--surface)",
                borderColor: "var(--danger)",
                color: "var(--danger)",
                boxShadow: "var(--shadow-2)",
                maxWidth: 520,
              }}
            >
              {error}
            </p>
          </div>
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
                ? { background: "var(--accent-btn)", color: "var(--accent-btn-text)", border: "none" }
                : {
                    background: "var(--surface)",
                    color: "var(--text)",
                    border: "1px solid var(--border-strong)",
                  }
            }
            onClick={onDone}
            title="随时可以进主界面，配置会保留"
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
