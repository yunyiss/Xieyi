import { useCallback, useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  discoverRuntimes,
  getAppInfo,
  getRuntimeStatus,
  cancelDownload,
  deleteRuntimePartial,
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
import { formatBytes, PresetModels } from "./PresetModels";

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
  /**
   * 底部浮条。
   *
   * ## 为什么要分 kind 和自动消失
   *
   * 早先这里只有一个 `error: string`，问题有三：
   *
   * 1. **不会消失**。只有下一次操作开始时才被 `setError("")` 清掉，
   *    所以用户取消下载后那句「已取消下载…」会一直挂在屏幕上。
   * 2. **取消被渲染成红色**。取消不是错误，是用户主动做的正常操作；
   *    后端专门为它加了 `cancelled` 阶段不报红字，前端却又把它染红了。
   * 3. 没有替换语义：两条提示同时来，后一条覆盖前一条，用户只看到一半。
   *
   * 现在用 kind 区分中性/错误，并统一自动消失。
   */
  const [notice, setNotice] = useState<{ kind: "info" | "error"; text: string } | null>(null);
  /** 自动消失的定时器。新提示到来时清掉旧的，避免前一条把后一条提前关掉。 */
  const noticeTimer = useRef<number | null>(null);

  /**
   * 显示一条浮条，几秒后自动消失。
   *
   * 错误停留久一点（用户需要读），中性提示短一些。
   */
  const showNotice = useCallback((kind: "info" | "error", text: string) => {
    if (noticeTimer.current !== null) {
      window.clearTimeout(noticeTimer.current);
    }
    setNotice({ kind, text });
    noticeTimer.current = window.setTimeout(
      () => setNotice(null),
      kind === "error" ? 8000 : 4000,
    );
  }, []);

  /** 立刻收起浮条（用户点 × 时）。 */
  const dismissNotice = useCallback(() => {
    if (noticeTimer.current !== null) {
      window.clearTimeout(noticeTimer.current);
      noticeTimer.current = null;
    }
    setNotice(null);
  }, []);

  // 组件卸载时清掉定时器，避免在已卸载的组件上 setState
  useEffect(
    () => () => {
      if (noticeTimer.current !== null) {
        window.clearTimeout(noticeTimer.current);
      }
    },
    [],
  );
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

  /**
   * 本次引擎下载是否已经收尾。
   *
   * 收尾有两条来路：`installRuntime` 的 Promise（正常）和「done」进度事件
   * （后端写完配置后发出）。两者谁先到不确定，只让先到的那条收尾；
   * 否则晚到的事件会把进度条重新设回「完成」，卡片永远停在那一步。
   */
  const downloadSettled = useRef(false);

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
      if (aliveRef.current) showNotice("error", String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  /**
   * 引擎下载的统一收尾。
   *
   * 必须同时由 **Promise 与「done」事件** 驱动：两者谁先到不确定，
   * 只等 Promise 的话，一旦回包迟到或丢失，界面就会永远停在
   * 「完成」，② 模型卡也不会解锁（用户实测偶发过，三次一次）。
   *
   * `refresh()` 读的是磁盘上的配置，不依赖任何回包，所以只要收尾
   * 走到一次，界面状态一定能对上。
   */
  const finishDownload = useCallback(
    async (rt?: RuntimeStatus) => {
      if (downloadSettled.current) return;
      downloadSettled.current = true;
      if (rt) setRuntime(rt);
      setProgress(null);
      setBusy("");
      await refresh();
      onChanged();
    },
    [refresh, onChanged],
  );

  useTauriEvent(onRuntimeProgress, (p) => {
    // 终态只负责收尾，不渲染进度：晚到的终态事件若照常 setProgress，
    // 进度条会重新出现并一直停在「完成」。
    if (p.stage === "done" || p.stage === "cancelled") {
      void finishDownload();
      return;
    }
    setProgress(p);
  });

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
    setBusy("engine");
    try {
      setRuntime(await setManualRuntime(path));
      await restartService();
      await refresh();
      onChanged();
    } catch (e) {
      showNotice("error", String(e));
    } finally {
      setBusy("");
    }
  };

  const pickEngine = async () => {
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
      showNotice("error", String(e));
    }
  };

  const download = async () => {
    downloadSettled.current = false;
    setBusy("download");
    try {
      // 回包只用来"顺带"更新状态；真正的收尾统一走 finally 的
      // finishDownload —— 回包迟到/丢失时也不会把界面卡在「完成」。
      setRuntime(await installRuntime("vulkan"));
    } catch (e) {
      // **取消不是错误。**
      //
      // 后端专门为取消加了独立的 `cancelled` 阶段，就是为了不让它走
      // 红字路径；前端这边若还染红，等于把后端的区分又抹平了。
      // 用户主动点「取消下载」，凭什么看到一条红色警告？
      //
      // 判据用文案包含「已取消」——后端的取消错误串固定在
      // `runtime.rs` 与 `presets.rs` 两处，都以「已取消下载」开头。
      // 这不是"用字符串当控制流"的理想解，但 `installRuntime` 返回的是
      // `String` 而非枚举，前端拿不到类型信息；真要根治得改命令签名，
      // 那是更大的改动，记在这里而不是现在做。
      //
      // 失败/取消后也必须收尾刷新一次：否则 `runtime.partialBytes`
      // 停在进入下载前的旧值，用户看不到残留，也拿不到清理入口。
      const msg = String(e);
      showNotice(msg.includes("已取消") ? "info" : "error", msg);
    } finally {
      await finishDownload();
    }
  };

  /**
   * 取消正在进行的引擎下载。
   *
   * 不做本地状态清理：`installRuntime` 的 Promise 会在取消后自己
   * reject，走上面 `download()` 的 catch。这里只负责把请求发出去，
   * 两层都改状态会打架。
   */
  const abortEngineDownload = async () => {
    try {
      await cancelDownload();
    } catch (e) {
      showNotice("error", String(e));
    }
  };

  /**
   * 清掉引擎包的下载残留。
   *
   * 与「取消」的区别：取消是"先停一下"（保留 `.part` 以便续传），
   * 这个按钮是"不要了"。没有它，用户取消几次之后那些半截包会永远
   * 占着磁盘，而且没有任何入口能查到。
   */
  const clearRuntimePartial = async () => {
    setBusy("clear-partial");
    try {
      await deleteRuntimePartial();
      await refresh();
    } catch (e) {
      showNotice("error", String(e));
    } finally {
      setBusy("");
    }
  };

  const pickDir = async () => {
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
      showNotice("error", String(e));
    } finally {
      setBusy("");
    }
  };

  /**
   * 预设模型下载完成后的收尾。
   *
   * **必须重启引擎**：`presets.ini` 是在 `spawn()` 里按当时的目录扫描
   * 结果写出来的。新下的 `.gguf` 不重启引擎永远不会被路由看见——
   * 用户下完了，翻译页还是说「没有模型」。
   *
   * 与 `pickDir` 的区别是这里不动模型目录、不改配置，只让引擎重扫一遍。
   * 引导层里这么做没有副作用：用户本来就在配置阶段，没有正在进行的翻译。
   */
  const onPresetDownloaded = useCallback(async () => {
    setBusy("model");
    try {
      await restartService();
      await refresh();
      onChanged();
    } catch (e) {
      showNotice("error", String(e));
    } finally {
      setBusy("");
    }
  }, [refresh, onChanged]);

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
  /**
   * 引擎是否正在下载/校验/解压。
   *
   * 有 `progress` 就说明流程在跑。用来决定「清理残留」要不要显示——
   * 正在下载时显示"清理上次的残留"会让人以为点它能停掉当前这次。
   */
  const engineDownloading = progress !== null;

  /**
   * 我们**自己下载的那份**引擎是否已经就位。
   *
   * 用来决定「下载推理引擎…」按钮要不要显示。
   *
   * 为什么不能只看 `engineReady`：用户手动指定 `D:\llama\llama-server.exe`
   * 时引擎也是就绪的，但 `runtime/` 目录下并没有我们下载的那一份，
   * 此时下载按钮应当保留——他可能想换成我们提供的 Vulkan 版。
   *
   * `backend === "manual"` 正是"手动指定"的判据（见 `runtime.rs` 的
   * `write_manual_info`）。
   */
  const hasDownloadedEngine =
    engineReady && runtime?.backend !== "manual" && Boolean(runtime?.backend);
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
        className="onboarding-pad mx-auto flex min-h-full w-full flex-col justify-center px-6 py-6"
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
        {/*
          单列 + 分步。

          ## 为什么不做两列（试过一次，撤了）

          引擎和模型是**依赖关系**：②需要①，没装引擎时模型卡片里什么都
          显示不出来。把依赖画成并排，右列必然大面积空白，看起来像没做完。

          UI/UX 规范里管这叫 Funnel 模式：**一次只讲一件事，用编号表达
          顺序**。编号 ①② 本来就是线性的，布局也该是线性的。

          早先也试过固定窄宽 + 居中，全屏时两侧留白太多；现在保持单列，
          但宽度放宽（见 index.css 的 .onboarding-pad），内容更舒展。
        */}
        <div className="onboarding-steps">
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
              {/*
                取消按钮。**只在真正在下载时出现**。

                为什么要它：实测国内加速代理约 90 KB/s，30.8 MB 要下 5 分钟。
                没有取消时用户只有两条路——干等，或者关掉窗口。而关窗口后
                `.part` 会留在磁盘上（其实下次能续传），但界面上看不见，
                用户会以为这次白下了。

                校验/解压/自检阶段不给取消：那些步骤不可中断，
                给了按钮反而让人以为点了能停。
              */}
              {progress.stage === "downloading" && (
                <button
                  className="btn mt-2 h-7 rounded-md border px-2 text-[11px]"
                  style={{
                    background: "transparent",
                    borderColor: "var(--border)",
                    color: "var(--text-muted)",
                  }}
                  onClick={() => void abortEngineDownload()}
                >
                  取消下载
                </button>
              )}
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
                {/*
                  装好之后就不再显示「下载推理引擎…」。

                  用户实测：装好引擎后这个按钮还在，再点一次会重新下
                  31 MB —— 国内 90 KB/s 要等 5 分钟，纯属白等。

                  但**手动指定**引擎时仍要保留它：那种情况下
                  `runtime/` 里并没有我们装的那一份，用户可能想换成
                  我们提供的 Vulkan 版（手动指定的未必是最优后端）。

                  后端 `install_runtime` 里也加了同样的闸门（查安装目录），
                  两层都拦：前端不给台阶，后端不给绕过。
                */}
                {!hasDownloadedEngine && (
                  <button
                    className="btn h-8 rounded-lg border px-3 text-[12px]"
                    style={btnStyle}
                    onClick={download}
                    disabled={busy !== ""}
                    title="从 GitHub 下载 llama.cpp 推理引擎（约 12–31 MB，视平台）"
                  >
                    下载推理引擎…
                  </button>
                )}
              </div>

              {/* 说清「下载」到底下的是什么，第一次用的人不会一头雾水 */}
              <p
                className="m-0 mt-2 text-[11px]"
                style={{ color: "var(--text-muted)" }}
              >
                下载的是 llama.cpp 官方预编译引擎：Windows/Linux 为 Vulkan 版
                （约 31 MB），macOS 为 Metal/CPU 版（约 12 MB）。
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

              {/*
                未完成下载的残留。

                引擎包下到 `runtime/downloads/*.zip.part`。取消之后那份数据
                是**故意保留**的（下次续传要用），但它此前完全不可见：
                不在任何列表里、启动时不扫描、界面上没有入口。
                31 MB 在 90 KB/s 下要 5 分钟，用户可能取消过好几次，
                磁盘上就躺着好几份半截包而他一无所知。

                这与模型的 `.part` 是**同一个缺陷**。模型那边上一轮已经
                修了（暂停态 + 删除按钮），引擎这边当时漏了。

                单列一行灰字 + 一个「清理」链接，不做成红字警告：
                残留不是错误，是"还没下完"的正常中间状态。
              */}
              {!engineDownloading && (runtime?.partialBytes ?? 0) > 0 && (
                <div
                  className="mt-2 flex items-center gap-2 border-t pt-2 text-[11px]"
                  style={{ borderColor: "var(--border)" }}
                >
                  <span style={{ color: "var(--text-muted)" }}>
                    上次下载留下 {formatBytes(runtime!.partialBytes ?? 0)}
                    ，下次会续传
                  </span>
                  <button
                    className="btn ml-auto h-6 rounded px-1.5 text-[11px]"
                    style={{
                      background: "transparent",
                      border: "1px solid var(--border)",
                      color: "var(--text-muted)",
                    }}
                    onClick={clearRuntimePartial}
                    disabled={busy !== ""}
                    title="删掉这份半截包，释放磁盘空间"
                  >
                    清理
                  </button>
                </div>
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
            /*
              空状态给**说明**就够了，不给按钮。

              曾经按 UI/UX 规范的 Empty State 条目（"Show helpful message
              and action"）在这里加过一个「回到第 ① 步」按钮，用户反馈
              「这是干什么用的，我没看懂」——因为：

              引导层总共两张卡，引擎卡**就在上面**，往上滑一眼就看到。
              一个滚到视线内已有内容的按钮，只会让人怀疑自己漏了什么。

              那条规范针对的是整屏空白、用户不知道从哪开始的场景，
              这里不适用。**规范要按场景用，不能照抄。**
            */
            <p
              className="m-0 text-[12px]"
              style={{ color: "var(--text-muted)" }}
            >
              请先完成上面的推理引擎配置
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
              {/*
                这里**不能**写"支持拖入窗口"。

                引导层没有 drop 处理——拖入只发生在主界面（Translate /
                Batch 页监听 Tauri 的 drag-drop 事件）。早先这句文案是从
                主界面照搬过来的，用户在引导层试了一次拖入，什么都没发生。

                宣传一个此时不存在的功能，比不宣传更糟：用户会以为
                "这软件坏了"。真要支持，得在这层加监听，那是另一件事。
              */}
              {/* 「没找到 .gguf」的提示已合并到标题右侧这一处，
                  不再在这里另起一行把卡片撑高。 */}

              {/*
                推荐模型：用户第一次面对「我没有模型」，此刻给下载最有用。

                这里 `onDownloaded` 让上层重启引擎——`presets.ini` 是在
                引擎启动时按当时的目录扫描结果写出来的，不重启的话新下的
                `.gguf` 永远不会被路由看见，用户下完还是用不了。
              */}
              <div
                className="mt-3 border-t pt-3"
                style={{ borderColor: "var(--border)" }}
              >
                <div className="mb-2 flex items-baseline gap-2">
                  <span className="section-title">推荐模型</span>
                  <span
                    className="text-[10px]"
                    style={{ color: "var(--text-muted)" }}
                  >
                    已实测可用
                  </span>
                </div>
                <PresetModels mode="featured" onDownloaded={() => void onPresetDownloaded()} />
              </div>
            </>
          )}
        </section>
        </div>
        </div>

        {/*
          底部浮条：错误与中性提示共用。

          做成不占布局的浮条（原来插在按钮上方，一出现就把「开始使用」
          往下推，视觉重心跟着跳）。

          ## 为什么分两种颜色
          
          取消下载不是错误。后端已经用独立的 `cancelled` 阶段把两者分开，
          前端若一律染红，等于把那个区分抹平——用户主动点了取消，
          却看到一条红色警告，观感是"我做错了什么"。

          ## 为什么能手动关
          
          自动消失是 4 秒（提示）/ 8 秒（错误），但用户可能想立刻清掉
          腾出视线。`pointer-events-auto` 只在浮条本身上开，外层保持
          none，这样它不会挡住下面的按钮。
        */}
        {notice && (
          <div
            className="pointer-events-none fixed inset-x-0 bottom-6 z-50 flex justify-center px-4"
            role={notice.kind === "error" ? "alert" : "status"}
          >
            <div
              className="pointer-events-auto flex items-center gap-2 rounded-lg border px-3 py-2 text-[12px]"
              style={{
                background: "var(--surface)",
                borderColor:
                  notice.kind === "error" ? "var(--danger)" : "var(--border-strong)",
                color:
                  notice.kind === "error" ? "var(--danger)" : "var(--text-muted)",
                boxShadow: "var(--shadow-2)",
                maxWidth: 520,
              }}
            >
              <span>{notice.text}</span>
              <button
                className="btn -mr-1 shrink-0 rounded px-1 text-[14px] leading-none"
                style={{ background: "transparent", color: "inherit", opacity: 0.6 }}
                onClick={dismissNotice}
                title="关闭"
                aria-label="关闭提示"
              >
                ×
              </button>
            </div>
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
