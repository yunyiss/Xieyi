import { useCallback, useEffect, useRef, useState } from "react";
// 这个组件的 props 里已经有 `open`（抽屉是否展开），
// 所以文件对话框的函数要改个名字，避免遮蔽。
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { evaluateFit } from "../lib/modelFit";
import {
  abort,
  discoverRuntimes,
  getGlossary,
  getRuntimeStatus,
  getSettings,
  installRuntime,
  listLocalModels,
  onRuntimeProgress,
  type RuntimeProgress,
  listRouterModels,
  openModelsDir,
  probeRam,
  probeTotalVram,
  probeVram,
  restartService,
  setGlossary,
  setGpuLayers,
  setManualRuntime,
  setModelParams,
  setModelChatCapable,
  setPreload,
  setPreloadModel,
  setModelsDir,
  setPinned,
  setTtl,
  type DiscoveredRuntime,
  type GlossaryEntry,
  type LocalModel,
  type MemoryInfo,
  type RuntimeProbe,
  type RuntimeStatus,
  type Settings,
} from "../lib/ipc";
import { useTauriEvent } from "../lib/useTauriEvent";
import { DownloadSourceSettings } from "./DownloadSourceSettings";
import { HardwareStatus } from "./HardwareStatus";
import { PresetModels } from "./PresetModels";
import { Select } from "./Select";
import { StorageInfo } from "./StorageInfo";
import { Switch } from "./Switch";

/**
 * 空闲释放档位。
 *
 * 分档而不是让用户填数字：这个东西用户说不清具体要多少秒，
 * 他要的是「省显存 / 平衡 / 少等待」三选一。
 *
 * 默认取**中档 5 分钟**：引擎只启动一次（约 0.55 秒），但模型每次闲置
 * 都要重载约 4 秒。实测「看一眼文档再回来」很容易超过 2 分钟，2 分钟档
 * 会让人反复等；而 10 分钟档在走开时要多占 4-5 GB 显存。
 */
const TTL_OPTIONS = [
  { value: 120, label: "空闲 2 分钟释放（省显存）" },
  { value: 300, label: "空闲 5 分钟释放（推荐）" },
  { value: 600, label: "空闲 10 分钟释放（少等待）" },
  { value: 0, label: "不自动释放" },
];

/** 默认空闲释放：5 分钟。见 `TTL_OPTIONS` 的说明。 */
const DEFAULT_TTL_SECS = 300;

/**
 * 把配置里读到的 TTL 补进选项列表。
 *
 * 老版本的配置可能存着已下线的档位（比如早先有 60 秒）。此时直接丢给
 * `Select` 会找不到匹配项、显示成占位符「—」——用户会以为设置坏了。
 *
 * 所以遇到不在表里的值就临时加一项。仍保留原值而不是悄悄改成默认：
 * 用户的配置不该被我们无声改写。
 */
function ttlOptionsWith(current: number) {
  if (TTL_OPTIONS.some((o) => o.value === current)) return TTL_OPTIONS;
  return [
    ...TTL_OPTIONS,
    { value: current, label: `空闲 ${current} 秒释放（旧设置）` },
  ].sort((a, b) => a.value - b.value);
}

const LAYER_OPTIONS = [
  { value: 1001, label: "自动（按显存）" },
  { value: 999, label: "全 GPU" },
  { value: 16, label: "混合 16 层" },
  { value: 0, label: "纯 CPU" },
];

/** 可选的上下文档位：不超过模型原生上限，且保留当前值。 */
function ctxOptions(model: LocalModel): number[] {
  const max = model.trainedCtx ?? 32768;
  const list = [2048, 4096, 8192, 16384, 32768, 65536].filter((v) => v <= max);
  if (!list.includes(model.ctxSize)) list.push(model.ctxSize);
  return list.sort((a, b) => a - b);
}

/**
 * 设置抽屉的四个页签。
 *
 * 「引擎」单独成页是 2026-11-17 拆的：原来「通用」一页同时装了主题、
 * 空闲释放、推理引擎、层数、存储位置五类东西，找一样要滚半天。
 * 现在按「偏好 / 引擎 / 模型 / 下载」分开。
 */
export type DrawerTab = "general" | "engine" | "models" | "network";

interface Props {
  open: boolean;
  onClose: () => void;
  onChanged: () => void;
  /**
   * 打开时默认停在哪个页签。
   *
   * 用于「只缺模型」的场景：翻译页的提示按钮把用户送到模型页，
   * 而不是让他打开抽屉后再自己找一遍。
   */
  initialTab?: DrawerTab;
}

const btnClass =
  "btn h-7 rounded-md border px-2 text-[11px] transition-colors disabled:opacity-50";

const btnStyle = {
  background: "var(--surface)",
  borderColor: "var(--border)",
  color: "var(--text)",
};

function stateLabel(state: string): { text: string; hot: boolean } {
  if (state === "loaded") return { text: "已加载", hot: true };
  if (state === "loading") return { text: "加载中", hot: true };
  if (state === "unloading") return { text: "卸载中", hot: true };
  return { text: "未加载", hot: false };
}

/** 引擎下载各阶段的中文说明。 */
function stageText(stage: string): string {
  switch (stage) {
    case "downloading":
      return "下载中";
    case "verifying":
      return "校验中";
    case "extracting":
      return "解压中";
    case "checking":
      return "自检中";
    case "done":
      return "完成";
    default:
      return "处理中";
  }
}

/** 右侧抽屉：模型列表与目录管理。 */
export function ModelDrawer({
  open,
  onClose,
  onChanged,
  initialTab = "general",
}: Props) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [local, setLocal] = useState<LocalModel[]>([]);
  /** 当前页签：engines 类参数与模型分开，避免一页塞太多。 */
  const [tab, setTab] = useState<DrawerTab>(initialTab);
  /** 当前推理引擎状态，用于「推理引擎」那一块。 */
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  /**
   * 引擎状态是否**已经读到过一次**。
   *
   * 与 `runtime !== null` 不等价：`runtime` 为 null 既可能是"还没读到"，
   * 也可能只是真的没有状态。而界面必须区分这两者——
   *
   * 早先直接用 `runtime?.installed` 决定按钮外观，于是在首次打开抽屉时，
   * 数据还在路上，`installed` 被当成 false，渲染出一个**深色主按钮**；
   * 几十毫秒后数据到了才变回次要样式。用户看到的是"一块深色闪一下"。
   *
   * 只改颜色是治标：真正的问题是**在知道答案之前就画了答案**。所以这里
   * 记住"是否已加载"，未加载时不渲染那个按钮，只留等高的占位。
   */
  const [runtimeLoaded, setRuntimeLoaded] = useState(false);
  /**
   * 「重新下载」是否停在待确认那一步。
   *
   * 两步确认：第一次点只是把按钮变成「确认重装？」，再点一次才真下载。
   * 不用系统模态框——理由见 `downloadEngine` 里那段注释。
   */
  const [confirmReinstall, setConfirmReinstall] = useState(false);
  /** 待确认状态的超时器：5 秒不点就退回普通按钮，避免卡在确认态。 */
  const confirmTimer = useRef<number | null>(null);

  /** 退出待确认态（并清掉定时器）。 */
  const clearReinstallConfirm = useCallback(() => {
    if (confirmTimer.current !== null) {
      window.clearTimeout(confirmTimer.current);
      confirmTimer.current = null;
    }
    setConfirmReinstall(false);
  }, []);

  // 卸载时清定时器，避免在已卸载的组件上 setState
  useEffect(
    () => () => {
      if (confirmTimer.current !== null) window.clearTimeout(confirmTimer.current);
    },
    [],
  );

  /** 引擎切换后的自检结果（成功显示版本，失败显示原因）。 */
  const [probeMsg, setProbeMsg] = useState<RuntimeProbe | null>(null);
  /** 扫描到的可切换引擎。 */
  const [discovered, setDiscovered] = useState<DiscoveredRuntime[]>([]);
  /**
   * 引擎下载进度。
   *
   * 之前设置里**没有下载入口**——「下载推理引擎」只在首启引导层有。
   * 用户删了引擎、或引导时点了「稍后配置」，就再也找不到下载按钮，
   * 只能手动去 GitHub 找文件。这里补上。
   */
  const [dlProgress, setDlProgress] = useState<RuntimeProgress | null>(null);
  const [states, setStates] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  /**
   * 中性提示（成功类）。
   *
   * 与 `error` 分开：错误用红色，成功不该染红。重装引擎耗时几十秒，
   * 完成后必须给一句明确的"成了"——否则用户无法判断这次点击有没有生效
   * （安装过程只在磁盘上覆盖目录与配置，界面上原本什么都不会变）。
   */
  const [notice, setNotice] = useState("");

  /*
   * 浮条自动消失。
   *
   * 错误留 8 秒（要读完，有些报错带两行说明），成功 4 秒。
   * 用户也可以点 × 立刻关掉。
   *
   * 为什么不留在原地等下次操作覆盖：早先就是那样，结果一条上世纪的报错
   * 会一直挂在面板上，用户以为当前状态出问题了。
   */
  useEffect(() => {
    if (!error) return;
    const t = window.setTimeout(() => setError(""), 8000);
    return () => window.clearTimeout(t);
  }, [error]);

  useEffect(() => {
    if (!notice) return;
    const t = window.setTimeout(() => setNotice(""), 4000);
    return () => window.clearTimeout(t);
  }, [notice]);
  const [glossaryText, setGlossaryText] = useState("");
  const [glossarySaved, setGlossarySaved] = useState(false);
  const [freeVram, setFreeVram] = useState<number | null>(null);
  /**
   * 硬件总量放在这里而不是 `HardwareStatus` 里。
   *
   * 抽屉是长期挂载的，而页签内容是条件渲染的——切走再切回会重新挂载
   * 子组件。数据若跟着子组件走，每次切回都要重新探测，界面先闪一下
   * 「检测中…」。硬件总量又极少变化，没必要跟着视图生命周期走。
   *
   * `undefined` = 还没读过；`null` = 读过但读不到。两者语义不同。
   */
  const [totalVramMb, setTotalVramMb] = useState<number | null | undefined>(undefined);
  const [ramInfo, setRamInfo] = useState<MemoryInfo | null | undefined>(undefined);
  /** 切换引擎后 +1，让「存储位置」重取当前的引擎路径。 */
  const [engineToken, setEngineToken] = useState(0);
  /** 改过上下文后需要重启引擎才生效 */
  const [needsRestart, setNeedsRestart] = useState(false);
  /** 「推荐模型」是否展开。**默认收起**——已下完的用户不需要它 */
  const [presetOpen, setPresetOpen] = useState(false);
  /** 还差几个预设没下。收起时用它透出这儿还有东西 */
  const [presetPending, setPresetPending] = useState(0);
  /** 退场动画期间仍然渲染，动画结束后再卸载 */
  const [closing, setClosing] = useState(false);

  useTauriEvent(onRuntimeProgress, setDlProgress);

  /**
   * 轻量轮询：只读配置与引擎状态。
   *
   * `listRouterModels` 内部走 `ensure()`，**会把引擎拉起来**。设置抽屉
   * 一打开就调它，等于「看设置」变成「启动推理引擎」——既慢又违背按需加载。
   * 所以只有引擎已经在跑时才问模型状态。
   */
  const refreshCore = useCallback(async () => {
    try {
      const s = await getSettings();
      setSettings(s);
      setRuntime(await getRuntimeStatus());
      setRuntimeLoaded(true);

      const terms = await getGlossary();
      setGlossaryText(
        (prev) => prev || terms.map((t) => `${t.from}=${t.to}`).join("\n"),
      );

      if (s.running) {
        try {
          const router = await listRouterModels();
          const map: Record<string, string> = {};
          router.forEach((m) => (map[m.id] = m.state));
          setStates(map);
        } catch {
          setStates({});
        }
      } else {
        setStates({});
      }
    } catch (e) {
      setError(String(e));
    }
  }, []);

  /**
   * 重量级探测：扫模型目录 + 问显存。
   *
   * 这两件事分别要读多个 GGUF 头、起子进程问显存，比读配置慢一个数量级。
   * 原来跟 2.5 秒轮询绑在一起，会让点击 / 滚动不定时卡一下。
   * 现在单独走 10 秒；失败时保留上一次结果，不覆盖成空。
   */
  const refreshHeavy = useCallback(async () => {
    try {
      setLocal(await listLocalModels());
    } catch {
      // 扫不到就保持上一次的列表
    }
    try {
      setFreeVram(await probeVram());
    } catch {
      // 显存读不到就保持上一次的值
    }
    try {
      // 内存可用量随系统占用变化，跟可用显存一起按 10 秒刷新
      setRamInfo(await probeRam());
    } catch {
      // 读不到就保持上一次的值，不把已有数字清空
    }
  }, []);

  /**
   * 硬件总量：只在打开抽屉时探一次，不跟着页签切换重跑。
   *
   * 显存总量与内存总量都几乎不变，没必要放进轮询；可用量由
   * `refreshHeavy` 按 10 秒刷新。两件事分开，切页签也不会闪「检测中」。
   */
  const refreshHardware = useCallback(async () => {
    setTotalVramMb(await probeTotalVram().catch(() => null));
  }, []);

  const refresh = useCallback(async () => {
    await Promise.all([refreshCore(), refreshHeavy()]);
  }, [refreshCore, refreshHeavy]);

  /** 「重新检测」：总量与可用显存一起重读。 */
  const redetectHardware = useCallback(() => {
    void refreshHardware();
    void refreshHeavy();
  }, [refreshHardware, refreshHeavy]);

  useEffect(() => {
    if (!open) return;
    void refresh();
    void refreshHardware();
    const core = window.setInterval(() => void refreshCore(), 2500);
    const heavy = window.setInterval(() => void refreshHeavy(), 10000);
    return () => {
      window.clearInterval(core);
      window.clearInterval(heavy);
    };
  }, [open, refresh, refreshCore, refreshHeavy, refreshHardware]);

  /**
   * 每次「打开」时应用外部指定的页签。
   *
   * 抽屉是常驻挂载的（只切显隐），`useState(initialTab)` 的初值只在
   * 首次渲染生效。若不在这里同步，第二次从外部带着不同页签打开时
   * 会停在用户上次手动切到的位置。
   *
   * 只在 `open` 变 true 时同步，不跟 `tab` 变化联动——否则用户手动
   * 切页签会被立刻弹回去。
   */
  useEffect(() => {
    if (open) setTab(initialTab);
    // 故意不依赖 tab：见上面注释
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, initialTab]);

  // 关闭时先播 170ms 退场动画再卸载，避免"啪一下消失"。
  // 用 wasOpen 记住"真的打开过"，否则首次挂载会误播一次退场。
  const wasOpen = useRef(false);
  useEffect(() => {
    if (open) {
      wasOpen.current = true;
      setClosing(false);
      return;
    }
    if (!wasOpen.current) return;
    wasOpen.current = false;
    setClosing(true);
    const timer = window.setTimeout(() => setClosing(false), 170);
    return () => window.clearTimeout(timer);
  }, [open]);

  // 按 Esc 关闭
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open && !closing) return null;

  const pinned = settings?.pinned ?? [];

  /**
   * 切换推理引擎。
   *
   * 后端会先跑 `--version` 自检，跑不起来的文件直接拒绝——
   * 否则会被记成"已安装"，直到真正翻译时才炸。
   * 成功后要把引擎重启，否则旧进程还在用旧的可执行文件。
   */
  const useEngine = async (path: string) => {
    setError("");
    setProbeMsg(null);
    setBusy("engine");
    try {
      const st = await setManualRuntime(path);
      setRuntime(st);
      setProbeMsg(st.probe ?? null);
      // 换引擎必须重启，否则推理还在跑旧的那个
      await restartService();
      await refresh();
      // 让「存储位置」重取，否则它还显示换之前的路径
      setEngineToken((n) => n + 1);
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  /** 用系统文件对话框挑一个 llama-server 可执行文件。 */
  const pickEngineFile = async () => {
    setError("");
    setProbeMsg(null);
    try {
      const picked = await openDialog({
        multiple: false,
        directory: false,
        title: "选择 llama-server 可执行文件",
        filters: [
          {
            name: "可执行文件",
            extensions: ["exe"],
          },
        ],
      });
      if (typeof picked !== "string") return;
      await useEngine(picked);
    } catch (e) {
      setError(String(e));
    }
  };

  /** 扫一遍磁盘，列出可以一键切换的引擎。 */
  const scanEngines = async () => {
    setError("");
    setBusy("engine");
    try {
      setDiscovered(await discoverRuntimes());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const togglePin = async (id: string, next: boolean) => {
    setError("");
    try {
      const list = await setPinned(id, next);
      setSettings((prev) => (prev ? { ...prev, pinned: list } : prev));
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  /**
   * 切换"能不能用在对话页"。
   *
   * 勾回去"自动"是没有单独按钮的——用户再点一次就得到与自动相反的结果，
   * 那反而是"手动"。所以这里只在值等于自动判定时清掉覆盖，
   * 让"手动改回原值"自然地恢复成自动。
   */
  const toggleChat = async (id: string, next: boolean) => {
    setError("");
    try {
      const m = local.find((x) => x.id === id);
      // 改回与自动一致的值，等于恢复自动识别，避免留下无意义的覆盖记录
      await setModelChatCapable(id, m && next === m.chatAuto ? null : next);
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  };

  const stopModel = async (id: string) => {
    setError("");
    setBusy(id);
    try {
      await abort(id);
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  /** 一键让位：卸载当前所有驻留模型，把显存还给前台程序。 */
  const releaseAll = async () => {
    setError("");
    setBusy("release");
    try {
      await abort();
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  /** 当前有多少个模型占着显存/正在加载，用于决定“一键让位”是否可用。 */
  const residentCount = local.filter((m) => {
    const s = states[m.id] ?? "unloaded";
    return s === "loaded" || s === "loading" || s === "unloading";
  }).length;

  /**
   * 下载推理引擎。
   *
   * 设置里原本没有这个入口——「下载推理引擎」只在首启引导层有。
   * 用户删了引擎、或引导时选了「稍后配置」，就再也找不到地方下载，
   * 只能自己去 GitHub 翻文件。补上这个缺口。
   *
   * 后端会按源顺序自动尝试（加速代理 → 官方 → 用户自填镜像），
   * 并在失败时自动回退，这里不用管源。
   */
  const downloadEngine = async () => {
    setError("");
    setNotice("");

    // **已装过时先确认。**
    //
    // 用户实测发现：引擎装好之后点「重新下载」，它会立刻闷头重下 31 MB，
    // 既不问一句、也没有任何"装成了"的反馈。连点几次就白下上百 MB，
    // 而且从界面上根本看不出到底装没装成（安装过程会在末尾覆盖
    // `config.json`，但那是磁盘上的事，界面不体现）。
    //
    // 确认框同时承担两个作用：拦住误点，以及**说明它是修复手段**——
    // 引擎目录损坏、缺 DLL、启动即崩时，覆盖重装是应用内唯一的自救路径。
    //
    // **不用系统确认框。** 早先这里调 `@tauri-apps/plugin-dialog` 的
    // `ask`，两个问题：
    //
    // 1. 系统弹窗由 OS 绘制，与整套自绘 UI 不搭——这也是本项目「所有下拉
    //    都用 `Select.tsx`、不用原生 `<select>`」的同一条理由。
    // 2. 它是模态的，会打断操作；而这件事远没到需要模态的程度。
    //
    // 改成**按钮自身两步确认**：第一次点把按钮变成「确认重装？」，再点一次
    // 才真正开始，5 秒不点自动退回。没有弹窗、没有模态、不打断。
    if (runtime?.installed && !confirmReinstall) {
      setConfirmReinstall(true);
      if (confirmTimer.current !== null) window.clearTimeout(confirmTimer.current);
      confirmTimer.current = window.setTimeout(() => {
        setConfirmReinstall(false);
        confirmTimer.current = null;
      }, 5000);
      return;
    }
    clearReinstallConfirm();

    setBusy("download-engine");
    setDlProgress(null);
    try {
      // `force = true`：这个按钮在装好之后叫「重新下载」，语义就是
      // 覆盖重装。引擎目录损坏 / 缺 DLL / 启动即崩时，这是应用内唯一的
      // 自救路径，后端那道「已装过就别再下」的闸门必须为它让路。
      setRuntime(await installRuntime("vulkan", true));
      setDlProgress(null);
      await restartService();
      await refresh();
      setEngineToken((n) => n + 1);
      onChanged();
      setNotice("引擎已重新安装");
    } catch (e) {
      setDlProgress(null);
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const restart = async () => {
    setError("");
    setBusy("restart");
    try {
      await restartService();
      await refresh();
      setNeedsRestart(false);
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  /**
   * 预设模型下载完成后的收尾。
   *
   * **必须重启引擎**：`presets.ini` 是 `spawn()` 时按当时的目录扫描结果
   * 写出来的，新下的 `.gguf` 不重启永远不会被路由看见——用户在设置里
   * 也切不过去，因为那个列表本身就来自引擎。
   *
   * 与引导层里的同名逻辑相比，这里**可能打断正在使用的模型**：
   * 抽屉随时能被打开，而某个模型可能正 warm 着。所以额外做两件事：
   *
   * 1. `setNeedsRestart(false)` —— 引擎刚重启过，那个「改了参数待重启」
   *    的提示该灭掉
   * 2. 保持 `busy` 状态，"正在重启…" 的文字让用户知道卡顿不是死机
   */
  const onPresetDownloaded = async () => {
    setError("");
    setBusy("restart");
    try {
      await restartService();
      await refresh();
      setNeedsRestart(false);
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  /** 改单个模型的上下文；引擎在启动时读 preset，所以必须重启才生效。 */
  const changeModelCtx = async (m: LocalModel, ctx: number) => {
    setError("");
    try {
      const saved = await setModelParams(m.id, ctx, m.thinking);
      setLocal((prev) =>
        prev.map((x) => (x.id === m.id ? { ...x, ctxSize: saved.ctx } : x)),
      );
      setNeedsRestart(true);
    } catch (e) {
      setError(String(e));
    }
  };

  /**
   * 换模型目录。
   *
   * 原来用 `window.prompt` 手打路径：Tauri 只 polyfill 了 alert/confirm，
   * prompt 是 WebView2 原生实现，既不能浏览目录也没法校验，打字极易出错。
   */
  const changeDir = async () => {
    setError("");
    try {
      const picked = await openDialog({
        multiple: false,
        directory: true,
        title: "选择存放 .gguf 的模型目录",
        defaultPath: settings?.modelsDir || undefined,
      });
      if (typeof picked !== "string") return;
      setBusy("dir");
      await setModelsDir(picked);
      await restartService();
      await refresh();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy("");
    }
  };

  const saveGlossary = async () => {
    setError("");
    const entries: GlossaryEntry[] = glossaryText
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => {
        const at = line.indexOf("=");
        if (at <= 0) return null;
        return {
          from: line.slice(0, at).trim(),
          to: line.slice(at + 1).trim(),
        };
      })
      .filter((e): e is GlossaryEntry => Boolean(e && e.from && e.to));

    try {
      await setGlossary(entries);
      setGlossarySaved(true);
      window.setTimeout(() => setGlossarySaved(false), 1500);
    } catch (e) {
      setError(String(e));
    }
  };

  const changeTtl = async (value: number) => {
    setError("");
    try {
      await setTtl(value);
      setSettings((prev) => (prev ? { ...prev, ttlSecs: value } : prev));
    } catch (e) {
      setError(String(e));
    }
  };

  const changeLayers = async (value: number) => {
    setError("");
    try {
      await setGpuLayers(value);
      setSettings((prev) => (prev ? { ...prev, gpuLayers: value } : prev));
    } catch (e) {
      setError(String(e));
    }
  };

  /**
   * 开关「启动时预加载模型」。
   *
   * 只写配置，不影响当前会话；下次启动才生效——所以提示里要写清楚，
   * 否则用户会以为勾了没反应。
   */
  const togglePreload = async (enabled: boolean) => {
    setError("");
    try {
      await setPreload(enabled);
      setSettings((prev) => (prev ? { ...prev, preload: enabled } : prev));
      setNotice(enabled ? "已开启启动预加载，下次启动生效" : "已关闭启动预加载");
    } catch (e) {
      setError(String(e));
    }
  };

  /** 指定预加载哪个模型；与开关一样，下次启动生效。 */
  const changePreloadModel = async (model: string) => {
    setError("");
    try {
      await setPreloadModel(model);
      setSettings((prev) => (prev ? { ...prev, preloadModel: model } : prev));
      setNotice("已更新预加载模型，下次启动生效");
    } catch (e) {
      setError(String(e));
    }
  };

  /**
   * 下拉里显示的预加载模型。
   *
   * 与后端选择顺序保持一致：显式指定 → 第一个常驻 → 第一个模型。
   * 只有存在于当前列表里的 id 才认，避免显示一个已被删除的名字。
   */
  const preloadId = (() => {
    const ids = new Set(local.map((m) => m.id));
    if (settings?.preloadModel && ids.has(settings.preloadModel)) {
      return settings.preloadModel;
    }
    const pinned = settings?.pinned.find((id) => ids.has(id));
    if (pinned) return pinned;
    return local[0]?.id ?? "";
  })();

  return (
    <>
      <div
        className="backdrop fixed inset-0 z-40"
        style={{
          background: "color-mix(in srgb, var(--text) 18%, transparent)",
          backdropFilter: "blur(2px)",
        }}
        onClick={onClose}
      />
      <aside
        className="drawer fixed bottom-2 right-2 top-2 z-50 flex w-[380px] flex-col overflow-hidden rounded-[16px]"
        style={{
          background: "var(--surface)",
          border: "1px solid var(--border)",
          boxShadow: "var(--shadow-3)",
          maxWidth: "calc(92vw - 16px)",
        }}
        data-closing={closing && !open}
      >
        <header
          className="flex shrink-0 items-center gap-2 border-b px-4 py-3"
          style={{ borderColor: "var(--border)" }}
        >
          <h2 className="m-0 text-[14px] font-medium">设置</h2>
          <button
            className="btn ml-auto flex h-7 w-7 items-center justify-center rounded-lg border"
            style={btnStyle}
            onClick={onClose}
            aria-label="关闭"
            title="关闭（Esc）"
          >
            ✕
          </button>
        </header>

        {/* 四页签：通用（偏好 + 硬件 + 存储）/ 引擎 / 模型与术语 / 下载源。
            全都堆一页时已经要滚半天，再加能力开关就彻底找不着了。 */}
        <div
          className="flex shrink-0 items-center gap-1 border-b px-3 py-1.5"
          style={{ borderColor: "var(--border)" }}
          role="tablist"
        >
          {(
            [
              { key: "general", label: "通用" },
              { key: "engine", label: "引擎" },
              { key: "models", label: "模型与术语" },
              { key: "network", label: "下载源" },
            ] as const
          ).map((t) => {
            const active = tab === t.key;
            return (
              <button
                key={t.key}
                role="tab"
                aria-selected={active}
                className="btn rounded-lg px-2.5 py-1 text-[12px]"
                style={{
                  background: active ? "var(--brand-soft)" : "transparent",
                  color: active ? "var(--brand-text)" : "var(--text-muted)",
                  fontWeight: active ? 500 : 400,
                }}
                onClick={() => setTab(t.key)}
              >
                {t.label}
              </button>
            );
          })}
          <span
            className="ml-auto text-[11px]"
            style={{ color: "var(--text-muted)" }}
          >
            {local.length} 个模型
          </span>
        </div>

        <div className="min-h-0 flex-1 overflow-auto px-4 py-3">
          {/* 通用：只剩系统信息（硬件 + 存储）。偏好与引擎参数都在「引擎」页。 */}
          {tab === "engine" && (
          <section className="mb-4">
            <div className="section-title mb-1.5">空闲释放</div>

            <div className="flex items-center gap-2 text-[12px]">
              <span className="w-[64px] shrink-0" style={{ color: "var(--text-muted)" }}>
                释放时机
              </span>
              <Select
                className="min-w-0 flex-1"
                size="sm"
                value={settings?.ttlSecs ?? DEFAULT_TTL_SECS}
                options={ttlOptionsWith(settings?.ttlSecs ?? DEFAULT_TTL_SECS)}
                onChange={(v) => changeTtl(v)}
                title="模型空闲多久后自动卸载，释放显存"
              />
            </div>

            <p className="m-0 mt-1 text-[11px] leading-relaxed" style={{ color: "var(--text-muted)" }}>
              模型闲置后自动卸载，把显存还给前台程序；引擎空壳不退出，下次请求会重新拉起模型。
            </p>
          </section>
          )}

          {/* 启动：预加载模型（默认关，显式开启才生效） */}
          {tab === "engine" && (
          <section className="mb-4">
            <div className="section-title mb-1.5">预加载</div>

            {/*
              用浅底卡片收住这一项，和下方「推理引擎」保持一致。
              裸行 + 右侧开关会让中间留出一大段空白，看起来像没画完。
            */}
            <div
              className="rounded-[10px] border p-2.5"
              style={{
                background: "var(--surface-muted)",
                borderColor: "var(--border)",
              }}
            >
              <div className="flex items-center gap-2 text-[12px]">
                <span>启动时预加载模型</span>
                <span className="ml-auto">
                  <Switch
                    checked={settings?.preload ?? false}
                    onChange={(v) => void togglePreload(v)}
                    label="启动时预加载模型"
                  />
                </span>
              </div>

              <p
                className="m-0 mt-1 text-[11px] leading-relaxed"
                style={{ color: "var(--text-muted)" }}
              >
                首次翻译几乎立即；启动会多几秒并占用显存，下次启动生效。
              </p>

              {/* 多个模型时让用户明确选；只有一个就直说，不摆一个假下拉 */}
              {settings?.preload && local.length > 1 && (
                <div className="mt-2 flex items-center gap-2 text-[12px]">
                  <span
                    className="w-[64px] shrink-0"
                    style={{ color: "var(--text-muted)" }}
                  >
                    预加载模型
                  </span>
                  <Select
                    className="min-w-0 flex-1"
                    size="sm"
                    value={preloadId}
                    options={local.map((m) => ({ value: m.id, label: m.id }))}
                    onChange={(v) => void changePreloadModel(v)}
                    title="启动时预加载哪个模型"
                  />
                </div>
              )}

              {settings?.preload && local.length === 1 && (
                <div className="mt-2 flex items-center gap-2 text-[12px]">
                  <span
                    className="w-[64px] shrink-0"
                    style={{ color: "var(--text-muted)" }}
                  >
                    预加载
                  </span>
                  <span className="min-w-0 flex-1 truncate" title={local[0].id}>
                    {local[0].id}
                  </span>
                </div>
              )}

              {settings?.preload && local.length === 0 && (
                <p
                  className="m-0 mt-2 text-[11px]"
                  style={{ color: "var(--text-muted)" }}
                >
                  模型目录里还没有 .gguf 文件
                </p>
              )}
            </div>
          </section>
          )}

          {/* 硬件与存储属于系统信息，不是引擎配置，留在「通用」。 */}
          {tab === "general" && (
            <HardwareStatus
              freeVramMb={freeVram}
              totalVramMb={totalVramMb}
              ram={ramInfo}
              onRedetect={redetectHardware}
            />
          )}

          {tab === "general" && <StorageInfo refreshToken={engineToken} />}

          {tab === "engine" && (
          <section className="mb-4">
            <div className="section-title mb-1.5">推理档位</div>

            <div className="flex items-center gap-2 text-[12px]">
              <span className="w-[64px] shrink-0" style={{ color: "var(--text-muted)" }}>
                层数
              </span>
              <Select
                className="min-w-0 flex-1"
                size="sm"
                value={settings?.gpuLayers ?? 1001}
                options={LAYER_OPTIONS}
                onChange={(v) => changeLayers(v)}
                title="给 GPU 分配多少层，自动档按空闲显存估算"
              />
            </div>

            <p className="m-0 mt-1 text-[11px]" style={{ color: "var(--text-muted)" }}>
              {settings?.gpuLayers === 1001
                ? "自动档按模型体积与当前可用显存分配层数。"
                : "固定层数不会随模型变化；显存放不下时会加载失败。"}
            </p>

            <div className="mt-2 flex items-center gap-2">
              <button
                className={btnClass}
                style={btnStyle}
                onClick={restart}
                disabled={busy === "restart"}
              >
                {busy === "restart" ? "重启中…" : "重启引擎"}
              </button>
              <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
                {needsRestart ? "上下文已改，重启后生效" : "档位改动后需重启生效"}
              </span>
            </div>
          </section>
          )}

          {tab === "engine" && (
          <section className="mb-4">
            <div className="flex items-baseline gap-2">
              <span className="section-title">推理引擎</span>
              <span className="text-[10px]" style={{ color: "var(--text-muted)" }}>
                可下载官方运行时，或指定本机已有的 llama-server
              </span>
            </div>

            <div
              className="mt-1.5 rounded-[10px] border p-2.5"
              style={{
                background: "var(--surface-muted)",
                borderColor: "var(--border)",
              }}
            >
              {/*
                未加载时**不说是/否**。

                这两行原本也会闪：先画「尚未安装推理引擎」，数据到了再换成
                真实路径。虽然只是一瞬，但那是**先说错、再改对**——用户如果
                恰好瞥见，会以为引擎掉了。

                这里用一句如实的「正在读取…」，等知道了再给结论。
              */}
              {!runtimeLoaded ? (
                <p
                  className="m-0 text-[12px]"
                  style={{ color: "var(--text-muted)" }}
                >
                  正在读取…
                </p>
              ) : (
                <>
                  <p
                    className="m-0 truncate text-[12px]"
                    title={runtime?.serverPath ?? ""}
                    style={{ color: "var(--text)" }}
                  >
                    {runtime?.installed
                      ? (runtime.serverPath ?? "")
                          .split(/[\\/]/)
                          .slice(-2)
                          .join(" / ")
                      : "尚未安装推理引擎"}
                  </p>
                  {/*
                    第二行平时显示「后端 · 版本」。

                    但在「待确认重装」时改用它说明这次操作要付出什么——
                    好处是**高度不变**：31 MB 与后果都写在这一行里，按钮
                    不用为了塞字变宽，也就不会把「扫描本机」挤到第二行。

                    早先把这个信息塞在按钮文案里（「确认重装？31 MB」），
                    结果按钮变宽 → 整行换行 → 排版跳了一下。挪到这里，
                    位置和高度都不动。
                  */}
                  {confirmReinstall ? (
                    <p
                      className="m-0 mt-0.5 text-[11px]"
                      style={{ color: "var(--brand-text)" }}
                    >
                      将覆盖安装，约 31 MB · 再点一次确认（5 秒后自动取消）
                    </p>
                  ) : (
                    <p
                      className="m-0 mt-0.5 text-[11px]"
                      style={{ color: "var(--text-muted)" }}
                    >
                      {runtime?.installed
                        ? `${runtime.backend ?? "未知"} · ${runtime.version ?? "未知版本"}`
                        : "尚未配置。可直接下载官方运行时，或指定本机已有的 llama-server"}
                    </p>
                  )}
                </>
              )}

              {probeMsg && (
                <p
                  className="m-0 mt-1.5 text-[11px] leading-relaxed"
                  style={{
                    color: probeMsg.ok ? "var(--brand-text)" : "var(--danger)",
                  }}
                >
                  {probeMsg.ok ? "✓ " : "✗ "}
                  {probeMsg.detail}
                </p>
              )}

              <div className="mt-2 flex flex-wrap items-center gap-1.5">
                {/*
                  下载入口放在最前：未配置时它是最该点的那个。
                  已装好时降级为次要操作（用户可能想换个后端重装）。

                  `runtimeLoaded` 之前**不渲染它**。原因是这个按钮的主/次
                  外观由 `runtime.installed` 决定，而首次打开抽屉时数据还在
                  路上，`installed` 会被当成 false → 画面先出现一块深色主
                  按钮，几十毫秒后再变成浅色次要按钮。

                  只改颜色是治标（深浅仍会变），根子是"在知道答案之前就画
                  了答案"。所以未加载时留一个等高占位：高度与 `h-8` 的按钮
                  一致，既不会闪，也不会让下面的元素跳一下。
                */}
                {runtimeLoaded ? (
                  <button
                    className={btnClass}
                    style={
                      confirmReinstall
                        ? {
                            // 待确认：给一点强调，但**不用红色**——
                            // 重装不是危险操作，红字会让人以为要出事
                            background: "var(--brand-soft)",
                            color: "var(--brand-text)",
                            borderColor: "var(--brand)",
                          }
                        : runtime?.installed
                          ? btnStyle
                          : {
                              background: "var(--accent-btn)",
                              color: "var(--accent-btn-text)",
                              border: "none",
                            }
                    }
                    onClick={downloadEngine}
                    disabled={busy !== ""}
                    title={
                      confirmReinstall
                        ? "再点一次即开始重新下载并覆盖安装（约 31 MB）"
                        : runtime?.installed
                          ? "重新下载并覆盖当前引擎。引擎损坏时可用它修复"
                          : "从官方或加速镜像下载 llama.cpp 运行时（Vulkan 版，约 31 MB）"
                    }
                  >
                    {busy === "download-engine"
                      ? "下载中…"
                      : confirmReinstall
                        ? "确认重装"
                        : runtime?.installed
                          ? "重新下载"
                          : "下载引擎"}
                  </button>
                ) : (
                  <span aria-hidden style={{ display: "inline-block", height: 28, width: 72 }} />
                )}
                <button
                  className={btnClass}
                  style={btnStyle}
                  onClick={pickEngineFile}
                  disabled={busy === "engine"}
                >
                  指定已有文件…
                </button>
                <button
                  className={btnClass}
                  style={btnStyle}
                  onClick={scanEngines}
                  disabled={busy === "engine"}
                >
                  {busy === "engine" ? "扫描中…" : "扫描本机"}
                </button>
              </div>

              {/* 下载进度：不显示的话用户只能看着按钮变灰干等 */}
              {dlProgress && (
                <div className="mt-2">
                  <div
                    className="mb-1 flex justify-between text-[10px]"
                    style={{ color: "var(--text-muted)" }}
                  >
                    <span>{stageText(dlProgress.stage)}</span>
                    <span className="tabular-nums">
                      {dlProgress.stage === "downloading"
                        ? `${dlProgress.percent.toFixed(1)}% · ${(
                            dlProgress.downloaded / 1048576
                          ).toFixed(1)} / ${(
                            dlProgress.total / 1048576
                          ).toFixed(1)} MB`
                        : ""}
                    </span>
                  </div>
                  <div
                    className="h-1.5 overflow-hidden rounded-full"
                    style={{ background: "var(--surface)" }}
                  >
                    <div
                      className="h-full rounded-full transition-[width] duration-200"
                      style={{
                        width: `${dlProgress.percent}%`,
                        background: "var(--brand)",
                      }}
                    />
                  </div>
                </div>
              )}

              {/* 扫出来的候选：点一下就能换，省得去文件对话框翻路径 */}
              {discovered.length > 0 && (
                <ul className="m-0 mt-2 flex list-none flex-col gap-1 p-0">
                  {discovered.map((d) => {
                    const current = d.path === runtime?.serverPath;
                    return (
                      <li key={d.path}>
                        <button
                          className="row w-full rounded-lg px-2 py-1.5 text-left"
                          data-selected={current}
                          disabled={busy === "engine" || current}
                          onClick={() => useEngine(d.path)}
                          title={d.path}
                        >
                          <span className="flex items-center gap-1.5">
                            <span className="min-w-0 flex-1 truncate text-[12px]">
                              {d.label}
                            </span>
                            <span
                              className="shrink-0 rounded px-1 py-0.5 text-[10px]"
                              style={{
                                background: "var(--brand-soft)",
                                color: "var(--brand-text)",
                              }}
                            >
                              {d.backend}
                            </span>
                            {current && (
                              <span
                                className="shrink-0 text-[10px]"
                                style={{ color: "var(--brand-text)" }}
                              >
                                当前
                              </span>
                            )}
                          </span>
                          <span
                            className="mt-0.5 block truncate text-[10px]"
                            style={{ color: "var(--text-muted)" }}
                          >
                            {d.version}
                          </span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>

          </section>
          )}

          {/*
            下载源单独一页。

            它是**横切关注点**：同时管引擎（GitHub）与模型（魔搭 / HF）
            两类下载。放在「通用」或「模型」页里都不贴切，而且两页各放
            一份会重复——自定义镜像、优先源这些设置以后还会增加。
          */}
          {tab === "network" && (
            <>
              <DownloadSourceSettings kind="engine" />
              <DownloadSourceSettings kind="model" />
            </>
          )}

          {tab === "models" && (
          <>
          {/* 术语表 */}
          <section className="mb-4">
            <div className="mb-1.5 flex items-baseline gap-2">
              <span className="section-title">术语表</span>
              <span className="text-[10px]" style={{ color: "var(--text-muted)" }}>
                每行一条：原文=译文
              </span>
            </div>

            <textarea
              className="field h-24 w-full resize-none p-2 text-[12px]"
              placeholder={"API=接口\nLLM=大语言模型"}
              value={glossaryText}
              onChange={(e) => {
                setGlossaryText(e.target.value);
                setGlossarySaved(false);
              }}
            />

            <div className="mt-1.5 flex items-center gap-2">
              <button className={btnClass} style={btnStyle} onClick={saveGlossary}>
                保存术语表
              </button>
              {glossarySaved && (
                <span className="text-[11px]" style={{ color: "var(--brand-text)" }}>
                  已保存
                </span>
              )}
            </div>
          </section>

          {/* 模型目录 */}
          <div className="mb-3">
            <div className="section-title mb-1">模型目录</div>
            <div className="flex items-center gap-2">
              <span
                className="min-w-0 flex-1 truncate text-[12px]"
                title={settings?.modelsDir}
              >
                {settings?.modelsDir ?? "—"}
              </span>
              <button
                className={btnClass}
                style={btnStyle}
                onClick={() => openModelsDir()}
              >
                打开
              </button>
              <button className={btnClass} style={btnStyle} onClick={changeDir}>
                切换
              </button>
            </div>
          </div>

          {/* 推荐模型下载。**默认收起**。

              与引导层的区别：引导层是用户第一次面对"我没有模型"，那里
              必须直接摊开（`mode="featured"`）；设置抽屉是回过头的用户，
              多半只是想改个参数，推荐区一直摊着反而占地方。

              收起的另一半理由：三档都下完之后，这块内容对他永久无用，
              却会一直把下面的本地模型列表往下推。

              标题行做成整行可点，不是只有一个小三角——点击区域小是
              "看起来像标题、其实是按钮"这类问题的常见来源。 */}
          <div
            className="mt-4 border-t pt-3"
            style={{ borderColor: "var(--border)" }}
          >
            <button
              className="btn flex w-full items-center gap-2 rounded-md px-1 py-0.5"
              style={{ background: "transparent", color: "var(--text)" }}
              onClick={() => setPresetOpen((v) => !v)}
              aria-expanded={presetOpen}
              title={presetOpen ? "收起推荐模型" : "展开推荐模型"}
            >
              <span
                aria-hidden
                style={{
                  display: "inline-block",
                  width: 0,
                  height: 0,
                  borderLeft: "3.5px solid transparent",
                  borderRight: "3.5px solid transparent",
                  borderTop: presetOpen ? "none" : "4px solid currentColor",
                  borderBottom: presetOpen ? "4px solid currentColor" : "none",
                  opacity: 0.5,
                }}
              />
              <span className="section-title">推荐模型</span>
              <span
                className="text-[10px]"
                style={{ color: "var(--text-muted)" }}
              >
                已实测可用
              </span>
              {/* 收起时把"还有几个没下"透出来，别让人忘了这儿有东西 */}
              {!presetOpen && (
                <span
                  className="ml-auto text-[10px]"
                  style={{ color: "var(--text-muted)" }}
                >
                  {presetPending > 0 ? `${presetPending} 个未下载` : "全部已下载"}
                </span>
              )}
            </button>

            {/*
              **始终挂载**，只用 CSS 隐藏。

              早先写成条件渲染（`presetOpen && <PresetModels …/>`），
              结果收起时组件根本没挂载——它就没机会把「还有几个没下」
              报上来，标题行右侧那个「N 个未下载」永远是空的。

              挂载着不展开的代价很小：它只查一次 list_presets 并订阅
              进度事件，都是已有的开销。
            */}
            <div
              className="mt-2"
              style={{ display: presetOpen ? undefined : "none" }}
            >
              <PresetModels
                mode="full"
                onDownloaded={() => void onPresetDownloaded()}
                onPendingChange={setPresetPending}
              />
            </div>
          </div>

          {/* 模型列表（单列） */}
          <div className="mb-1 mt-4 flex items-center gap-2">
            <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
              共 {local.length} 个模型
            </span>
            <div className="ml-auto flex items-center gap-2">
              <button
                className={btnClass}
                style={btnStyle}
                onClick={releaseAll}
                disabled={busy === "release" || residentCount === 0}
                title={
                  residentCount === 0
                    ? "当前没有驻留模型"
                    : "卸载所有驻留模型，把显存还给前台程序"
                }
              >
                {busy === "release" ? "处理中…" : "一键让位"}
              </button>
              <button
                className={btnClass}
                style={btnStyle}
                onClick={refresh}
              >
                刷新
              </button>
            </div>
          </div>

          <ul className="m-0 flex list-none flex-col gap-2 p-0">
            {local.length === 0 && (
              <li className="text-[12px]" style={{ color: "var(--text-muted)" }}>
                目录里还没有 .gguf 文件
              </li>
            )}

            {local.map((m) => {
              const state = states[m.id] ?? "unloaded";
              const { text, hot } = stateLabel(state);
              const isPinned = pinned.includes(m.id);
              const working = state === "loading" || state === "unloading";

              // 兼容性只在硬件数据齐全时给结论；读不到就不占位、不编数字。
              const fit =
                totalVramMb !== undefined && ramInfo?.totalMb
                  ? evaluateFit({
                      sizeBytes: m.sizeMb * 1024 * 1024,
                      ctxLength: m.ctxSize,
                      totalVramMb,
                      totalRamMb: ramInfo.totalMb,
                      kvShape: m.kvShape,
                    })
                  : null;
              const fitColor =
                fit?.level === "green"
                  ? "var(--brand-text)"
                  : fit?.level === "yellow"
                    ? "var(--text-muted)"
                    : "var(--danger)";
              const fitLabel =
                fit?.level === "green"
                  ? "可全速"
                  : fit?.level === "yellow"
                    ? "部分内存"
                    : "内存不足";

              return (
                <li
                  key={m.id}
                  className="rounded-[10px] border p-2.5 transition-colors"
                  style={{
                    background: "var(--surface-muted)",
                    borderColor: isPinned
                      ? "color-mix(in srgb, var(--brand) 55%, var(--border))"
                      : "var(--border)",
                  }}
                >
                  <div className="flex items-center gap-2">
                    <span
                      className="min-w-0 flex-1 truncate text-[13px] font-medium"
                      title={m.id}
                    >
                      {m.id}
                    </span>
                    <span
                      className="shrink-0 rounded-full px-1.5 py-0.5 text-[10px]"
                      style={{
                        background: hot ? "var(--brand-soft)" : "transparent",
                        color: hot ? "var(--brand-text)" : "var(--text-muted)",
                        border: hot ? "none" : "1px solid var(--border)",
                      }}
                    >
                      {text}
                    </span>
                  </div>

                  {/* 家族与能力：用户放一堆模型时，这里能一眼看出谁能干什么。
                      不可用模型（如 Seed-X）直接说明原因，别让他自己试出来。 */}
                  <div className="mt-1 flex flex-wrap items-center gap-1.5">
                    {m.family && m.family !== "通用" && (
                      <span
                        className="rounded px-1.5 py-0.5 text-[10px]"
                        style={{
                          background:
                            "color-mix(in srgb, var(--brand) 12%, transparent)",
                          color: "var(--brand-text)",
                        }}
                        title={`识别为 ${m.family} 家族，翻译时会用该家族的推荐调用方式`}
                      >
                        {m.family}
                      </span>
                    )}
                    {!m.unsupportedReason && (
                      <span
                        className="rounded px-1.5 py-0.5 text-[10px]"
                        style={{
                          border: "1px solid var(--border)",
                          color: "var(--text-muted)",
                        }}
                        title={
                          m.chatCapable
                            ? "翻译页、批量页、对话页都可以用"
                            : "只用于翻译与批量；对话页不显示（它会把提问本身翻译掉）"
                        }
                      >
                        {m.chatCapable ? "翻译 + 对话" : "仅翻译"}
                      </span>
                    )}
                  </div>

                  {m.unsupportedReason && (
                    <p
                      className="m-0 mt-1 text-[11px] leading-relaxed"
                      style={{ color: "var(--danger)" }}
                    >
                      {m.unsupportedReason}
                    </p>
                  )}

                  <div className="mt-1 flex items-center gap-2">
                    <span className="text-[11px]" style={{ color: "var(--text-muted)" }}>
                      {m.sizeMb} MB
                    </span>

                    {fit && fit.level !== "unknown" && (
                      <span
                        className="inline-flex items-center gap-1 text-[11px]"
                        style={{ color: fitColor }}
                        data-fit={fit.level}
                        title={fit.text}
                      >
                        <span
                          className="inline-block h-1.5 w-1.5 rounded-full"
                          style={{ background: fitColor }}
                        />
                        {fitLabel}
                      </span>
                    )}

                    {/* 能力开关：勾上就能进对话页。自动识别不准时用它改，
                        不必去动模型文件名——这是识别的兜底。 */}
                    <label
                      className="ml-auto flex cursor-pointer items-center gap-1 text-[11px]"
                      title={
                        m.chatOverridden
                          ? `已手动设置（自动判定为${m.chatAuto ? "可用" : "不可用"}），点击恢复自动`
                          : `自动判定为${m.chatAuto ? "可用" : "不可用"}；手动改过之后以你为准`
                      }
                    >
                      <input
                        type="checkbox"
                        checked={m.chatCapable}
                        onChange={(e) => toggleChat(m.id, e.target.checked)}
                        className="accent-[var(--brand)]"
                      />
                      对话
                      {m.chatOverridden && (
                        <span style={{ color: "var(--brand-text)" }}>·手动</span>
                      )}
                    </label>

                    <label className="flex cursor-pointer items-center gap-1 text-[11px]">
                      <input
                        type="checkbox"
                        checked={isPinned}
                        onChange={(e) => togglePin(m.id, e.target.checked)}
                        className="accent-[var(--brand)]"
                      />
                      常驻
                    </label>

                    {(state === "loaded" || working) && (
                      <button
                        className={btnClass}
                        style={btnStyle}
                        onClick={() => stopModel(m.id)}
                        disabled={busy === m.id}
                      >
                        {busy === m.id
                          ? "处理中…"
                          : state === "loading"
                            ? "停止加载"
                            : "卸载"}
                      </button>
                    )}
                  </div>

                  {/* 单个模型的上下文：进程级参数，只能在这里改；
                      思考强度在对话页输入区选，不在这里重复一份 */}
                  <div className="mt-1.5 flex items-center gap-1.5 text-[11px]">
                    <span className="shrink-0" style={{ color: "var(--text-muted)" }}>
                      上下文
                    </span>
                    <Select
                      className="min-w-0 flex-1"
                      size="sm"
                      value={m.ctxSize}
                      options={ctxOptions(m).map((v) => ({ value: v, label: String(v) }))}
                      onChange={(v) => changeModelCtx(m, v)}
                      title={
                        m.trainedCtx
                          ? `模型原生上限 ${m.trainedCtx}`
                          : "未能从 GGUF 读出原生上限"
                      }
                    />
                  </div>

                  {!m.supportsThinking && (
                    <p
                      className="m-0 mt-1 text-[10px]"
                      style={{ color: "var(--text-muted)" }}
                    >
                      该模型不支持思考，对话页不会显示思考强度
                    </p>
                  )}

                  {m.trainedCtx != null && (
                    <p
                      className="m-0 mt-1 text-[10px]"
                      style={{ color: "var(--text-muted)" }}
                    >
                      原生上下文 {m.trainedCtx}
                    </p>
                  )}
                </li>
              );
            })}
          </ul>
          </>
          )}

          {/*
            提示做成**浮在面板底部**的浮条，不再插在内容流里。

            原因（用户实测）：错误原本渲染在滚动内容的最末尾，面板一滚，
            报错就跑到视野外了——「解压失败: i/o error」就是这么被错过的。
            而且它还会把后面的内容往下推，出现/消失时整块跳。

            浮条不受滚动影响，任何位置的操作报错都能立刻看到。

            自动消失的时长按类型区分：错误留久一点（要读完），成功短一些。
            */}
          {(error || notice) && (
            <div className="pointer-events-none absolute inset-x-3 bottom-3 z-10">
              <div
                className="popover pointer-events-auto flex items-start gap-2 px-3 py-2 text-[12px]"
                style={{
                  background: "var(--surface)",
                  borderColor: error ? "var(--danger)" : "var(--border-strong)",
                  color: error ? "var(--danger)" : "var(--text)",
                }}
                role={error ? "alert" : "status"}
              >
                <span className="min-w-0 flex-1 whitespace-pre-line break-words">
                  {error || notice}
                </span>
                <button
                  className="btn -mr-1 shrink-0 rounded px-1 text-[14px] leading-none"
                  style={{
                    background: "transparent",
                    color: "inherit",
                    opacity: 0.6,
                  }}
                  onClick={() => {
                    setError("");
                    setNotice("");
                  }}
                  aria-label="关闭提示"
                  title="关闭"
                >
                  ×
                </button>
              </div>
            </div>
          )}
        </div>
      </aside>
    </>
  );
}
