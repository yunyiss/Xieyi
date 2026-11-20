import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

export interface AppInfo {
  os: string;
  arch: string;
  version: string;
  dataDir: string;
  modelsDir: string;
  /**
   * 用户是否**显式设过**模型目录。
   *
   *  永远有值（配置或程序目录下的默认 models），
   * 不能靠它判断「用户做过选择」。
   */
  modelsDirConfigured: boolean;
  runtimeVersion: string | null;
  /**
   * 是否运行在便携模式（程序目录下有 data/ 目录）。
   *
   * 界面据此显示模式标签：用户把 exe 单独拖走后，
   * 会发现设置全没了，却不知道原因是「已经不在便携目录里了」。
   */
  portable: boolean;
}

export interface RuntimeStatus {
  installed: boolean;
  version: string | null;
  backend: string | null;
  serverPath: string | null;
  modelsDir: string;
  /** 自检结果；只有显式「切换引擎」时才返回 */
  probe?: RuntimeProbe | null;
  /**
   * 未完成的引擎包下载残留在磁盘上的字节数。0 表示没有残留。
   *
   * 引擎包下到 `runtime/downloads/*.zip.part`。取消之后那份数据是
   * **故意保留**的（下次续传要用），但它此前完全不可见——不在任何列表
   * 里、启动时不扫描、界面上没有入口。用户取消过几次，磁盘上就静静
   * 躺着几份半截包。
   *
   * 与模型 `.part` 是同一个缺陷；模型那边有 `PresetStatus.partialBytes`
   * 和「删除」按钮，引擎这边靠这个字段补齐。
   *
   * 标为可选：后端始终返回它，但前端也应当能在与旧后端配对时安全工作
   * （读取处一律 ）。
   */
  partialBytes?: number;
}

/** 对某个 llama-server 的自检结果。 */
export interface RuntimeProbe {
  ok: boolean;
  /** 版本行（成功时）或失败原因 */
  detail: string;
}

/** 磁盘上已发现、可直接切换过去的引擎。 */
export interface DiscoveredRuntime {
  path: string;
  /** 目录名，便于辨认 */
  label: string;
  /** cuda / vulkan / cpu / unknown */
  backend: string;
  version: string;
}

export interface ServiceStatus {
  running: boolean;
  port: number | null;
  loaded: string[];
  modelsDir: string;
}

export interface RouterModel {
  id: string;
  state: string;
}

export interface RuntimeProgress {
  stage: "downloading" | "verifying" | "extracting" | "checking" | "done";
  downloaded: number;
  total: number;
  percent: number;
}

export interface Settings {
  ttlSecs: number;
  gpuLayers: number;
  pinned: string[];
  modelsDir: string;
  running: boolean;
  port: number | null;
  /** 启动时预加载模型；默认关闭，下次启动生效 */
  preload: boolean;
  /** 预加载哪个模型；`null` 表示按「常驻 → 第一个」回退 */
  preloadModel: string | null;
}

export interface LocalModel {
  id: string;
  path: string;
  sizeMb: number;
  /** 当前生效的上下文；改完需重启引擎 */
  ctxSize: number;
  /** 模型原生上限（读 GGUF 元数据），读不到为 null */
  trainedCtx: number | null;
  /** 对话模板是否支持思考（读 GGUF 里的 chat_template 判断） */
  supportsThinking: boolean;
  thinking: ThinkingMode;
  /**
   * 能否在对话页使用。
   *
   * 翻译专用模型为 false——实测它们会把用户的问题本身翻译掉、或编造身份，
   * 放进对话页只会让人以为工具坏了。判定依据来自后端读取 GGUF 元数据。
   */
  chatCapable: boolean;
  /** 家族名，用于徽章（"Hy-MT2"、"TranslateGemma"…） */
  family: string;
  /** 完全用不了时的原因；为空表示可用 */
  unsupportedReason: string | null;
  /** 自动识别的结果（不受手动覆盖影响），用于在界面上说明"自动判定为…" */
  chatAuto: boolean;
  /** 用户是否手动改过「能不能对话」 */
  chatOverridden: boolean;
}

/**
 * 思考档位，与 llama.cpp Web UI 一致：
 * default 交给模型、off 关闭、low/medium/high 分别限 512/2048/8192 token、max 不限。
 * 能否生效取决于模型对话模板是否支持 enable_thinking。
 */
export type ThinkingMode = "default" | "off" | "low" | "medium" | "high" | "max";

export interface GlossaryEntry {
  from: string;
  to: string;
}

export interface TranslateProgress {
  done: number;
  total: number;
}

/** 质检报告：某段译文被查出问题（后端已自动带纠正提示重试一次）。 */
export interface QualityReport {
  requestId: string;
  /** 出问题那段的开头几个字 */
  segment: string;
  /** 检出了什么，例如「疑似截断（原文 120 字，译文仅 8 字）」 */
  found: string;
  /**
   * 检出问题的原因码（去重），如 `untranslated`、`wrong-script`。
   *
   * 界面据此选择展示哪条提示，不去解析 `found` 里的中文——
   * 文案会改，解析字符串太脆。
   */
  codes: string[];
  /** 重试后是否修好 */
  fixed: boolean;
}

/**
 * 模型状态事件。`loading` / `warming` 时附带加载阶段信息，用于显示进度。
 * `percent` 是按模型体积估算的，不是 llama.cpp 的真实进度。
 */
export interface ModelStatePayload {
  model: string;
  state: string;
  phase?: string;
  elapsedMs?: number;
  estimateMs?: number;
  percent?: number;
}

export interface TranslateRequest {
  requestId: string;
  model: string;
  source: string;
  target: string;
  text: string;
  /**
   * 跳过缓存重新生成。对应界面上的「重新翻译」。
   *
   * 默认 false：同一次运行内翻过同一段会秒出（内存缓存）。
   * 置 true 时不读缓存、重新请求模型——否则用户点了会拿到一模一样的译文，
   * 看起来像按钮坏了。
   */
  skipCache?: boolean;
}

/**
 * 翻译结果。
 *
 * `fromCache` 表示整段文本**全部**来自缓存。界面上只在它为 true 时
 * 提示「来自缓存」，让用户知道这不是新生成的结果。
 */
export interface TranslateResult {
  text: string;
  fromCache: boolean;
}

export interface ChatMessage {
  /** 消息唯一 id。动作定位与 React key 都用它，不再依赖数组下标。 */
  id: string;
  role: "user" | "assistant" | "system";
  /** 当前显示的内容。等同于 versions[active]，保留单值是为了所有旧逻辑不用改。 */
  content: string;
  /** 思考型模型的思维链：只用于显示，发请求时会被忽略 */
  reasoning?: string;
  /** 消息状态；旧会话没有这个字段，读入时按 ready 处理 */
  status?: "ready" | "pending" | "error" | "stopped";
  /**
   * 同一条回复的多个版本（重新生成时追加，不覆盖）。
   *
   * 只在"重新生成"时产生：第一次生成不写这个字段，旧会话也没有。
   * 长度 > 1 表示这条消息有分支，界面显示 1/2 这样的计数器。
   */
  versions?: MessageVersion[];
  /** 当前显示第几版（versions 的下标）。没有 versions 时视为 0。 */
  active?: number;
}

/** 一条回复的一个版本。 */
export interface MessageVersion {
  content: string;
  /** 该版本的思维链（如果有） */
  reasoning?: string;
}

export interface ChatRequest {
  requestId: string;
  model: string;
  messages: ChatMessage[];
  /** 本次对话的思考档位覆盖；不传则用该模型在设置里的档位 */
  thinking?: ThinkingMode;
}

export interface Session {
  id: string;
  title: string;
  model: string;
  updatedAt: number;
  messages: ChatMessage[];
}

export const getAppInfo = () => invoke<AppInfo>("app_info");
/**
 * 让系统窗口（标题栏）跟随界面主题。
 *
 * Tauri 底层已封装三平台：Windows 用 DWM、Linux 用
 * `gtk-application-prefer-dark-theme`、macOS 用 `NSAppearance`。
 *
 * 传 `"system"` 而不是解析成具体颜色——后端会映射成 `None`（跟随系统），
 * 这样 WebView 的 `prefers-color-scheme` 保持与真实 OS 同步，
 * 前端媒体查询能自动响应系统主题变化。
 */
export const syncWindowTheme = (theme: "light" | "dark" | "system") =>
  invoke<void>("sync_window_theme", { theme }).catch(() => {
    // 标题栏配色是锦上添花，失败不影响使用，也不该弹错误
  });
export const getRuntimeStatus = () => invoke<RuntimeStatus>("runtime_status");
/**
 * 下载并安装推理引擎。
 *
 * `force` 为 true 时**绕过「已装过」的拦截**，重新下载并覆盖安装。
 *
 * 这是设置里「重新下载」用的：拦截的初衷是防止"装好了还点下载白等
 * 31 MB"，但反过来它就是**修复手段**——引擎目录损坏、缺 DLL、启动即崩
 * 时，重下一遍是最直接的自救方式。引导层的下载按钮在装好后会隐藏，
 * 走的永远是 `force = false`。
 */
export const installRuntime = (backend: string, force = false) =>
  invoke<RuntimeStatus>("install_runtime", { backend, force });
export const setManualRuntime = (serverPath: string) =>
  invoke<RuntimeStatus>("set_manual_runtime", { serverPath });

/**
 * 删掉未完成的引擎包下载，返回释放的字节数。
 *
 * 与「取消」的区别：取消是「先停一下」保留数据以便续传；删除是「不要了」。
 * 没有这个入口，取消几次之后那些半截包会永远占着磁盘且无从察觉。
 */
export const deleteRuntimePartial = () => invoke<number>("delete_runtime_partial");
/** 扫一遍磁盘，找现成可切换的 llama-server。 */
export const discoverRuntimes = () =>
  invoke<DiscoveredRuntime[]>("discover_runtimes");

export const getServiceStatus = () => invoke<ServiceStatus>("service_status");
export const startService = () => invoke<ServiceStatus>("start_service");
export const stopService = () => invoke<void>("stop_service");
export const listRouterModels = () => invoke<RouterModel[]>("list_router_models");
export const openModelsDir = () => invoke<string>("open_models_dir");
/** 打开当前引擎所在目录（手动指定的可以不在数据目录里）。 */
export const openEngineDir = () => invoke<string>("open_engine_dir");
/**
 * 在系统文件管理器里打开数据目录的某个子目录。
 *
 * `root` 是整个数据根目录（删掉它等于还原出厂），其余是它下面的子项。
 * 后端只接受这几个名字，不接受任意路径。
 */
export const openDataSubdir = (
  name: "root" | "sessions" | "runtime" | "logs" | "cache",
) => invoke<string>("open_data_subdir", { name });
export const setModelsDir = (path: string) =>
  invoke<string>("set_models_dir", { path });
export const setTtl = (seconds: number) => invoke<void>("set_ttl", { seconds });
export const setGpuLayers = (layers: number) =>
  invoke<void>("set_gpu_layers", { layers });
/** 开关「启动时预加载模型」；下次启动生效。 */
export const setPreload = (enabled: boolean) =>
  invoke<void>("set_preload", { enabled });
/** 指定预加载哪个模型；下次启动生效。 */
export const setPreloadModel = (model: string) =>
  invoke<void>("set_preload_model", { model });
/** 保存单个模型的上下文与思考开关；上下文改动需重启引擎。 */
export const setModelParams = (model: string, ctx: number, thinking: ThinkingMode) =>
  invoke<{ ctx: number; thinking: ThinkingMode }>("set_model_params", {
    model,
    ctx,
    thinking,
  });
/**
 * 手动指定某个模型能不能用在对话页。传 `null` 恢复自动识别。
 *
 * 自动识别靠读 GGUF 特征，改了文件名或遇到冷门模型时会认错，
 * 这个开关就是给用户兜底的——不用去改模型文件。
 */
export const setModelChatCapable = (model: string, capable: boolean | null) =>
  invoke<void>("set_model_chat_capable", { model, capable });
export const setPinned = (model: string, pinned: boolean) =>
  invoke<string[]>("set_pinned", { model, pinned });
export const getSettings = () => invoke<Settings>("get_settings");
export const listLocalModels = () => invoke<LocalModel[]>("list_local_models");

/**
 * 只数模型目录里有几个 .gguf，**不启动引擎**。
 *
 * 启动时判断「要不要弹引导层」只需要这个数量。用 `listRouterModels`
 * 会连带 `ensure()` 拉起 llama-server——那是 4-6 秒的等待，且用户
 * 可能只是想看一眼界面。见 service.rs 里 `count_local_models` 的注释。
 */
export const countLocalModels = () => invoke<number>("count_local_models");

/**
 * 一个预设模型的下载状态。
 *
 * `downloaded` 由后端按**磁盘实际文件 + 大小匹配**判断，不是配置记录——
 * 用户手动删了文件也会正确显示为未下载。
 */
export interface PresetStatus {
  id: string;
  name: string;
  summary: string;
  sizeBytes: number;
  /** 建议显存（MB）。显卡不够时提前提示，避免"引擎启动失败"的天书报错 */
  vramHintMb: number;
  /** 模型家族，与设置里显示的家族徽章一致 */
  family: string;
  downloaded: boolean;
  path: string | null;
  /**
   * 未完成下载已占用的字节数（`.part` 的大小）。0 表示没有残留。
   *
   * 与 `downloaded` 互斥。大于 0 时界面显示「已暂停」，
   * 并给出续传与删除两个入口。
   */
  partialBytes: number;
  /** 未完成下载的期望总字节数；0 表示未知（`.part.meta` 没写或读不到） */
  partialTotal: number;
}

/** 预设下载进度事件。`stage` 为 `failed` 时 `error` 有值。 */
export interface PresetProgress {
  id: string;
  /**
   * `cancelled` 是独立的阶段，**不是失败**：界面上不该报红字，
   * 而应显示成「已暂停 · 可续传」。
   */
  stage: "downloading" | "verifying" | "done" | "cancelled" | "failed";
  downloaded: number;
  total: number;
  percent: number;
  error?: string;
}

/**
 * 下载源配置。
 *
 * `preferred` 为 `null` 表示按默认顺序（模型：魔搭 → HF；
 * 引擎：加速代理 → 官方）。
 * `customMirror` 是用户自填的镜像基地址，为空表示没填。
 */
export interface DownloadSourceConfig {
  /** 类别：`engine` 或 `model` */
  kind: "engine" | "model";
  preferred: string | null;
  customMirror: string | null;
}

export const getDownloadSourceConfig = (kind: "engine" | "model") =>
  invoke<DownloadSourceConfig>("get_download_source_config", { kind });

/**
 * 保存下载源配置。
 *
 * `preferred` 传 `null` 恢复默认顺序；`customMirror` 传空串清除。
 * 后端会校验：镜像必须以 http(s):// 开头；偏好源必须是该类别支持的源
 * （拿模型源去配引擎会被拒绝，避免"存了却不生效"的静默失败）。
 */
export const setDownloadSourceConfig = (
  kind: "engine" | "model",
  preferred: string | null,
  customMirror: string | null,
) =>
  invoke<void>("set_download_source_config", { kind, preferred, customMirror });

export const listPresets = () => invoke<PresetStatus[]>("list_presets");

/**
 * 下载一个预设模型。返回下载后的实际路径。
 *
 * 会按源顺序自动尝试并回退；成功后记住那次用的源。
 * **不自动切换当前模型**——那会打断用户正在做的事，且需要重启引擎。
 */
export const downloadPreset = (id: string) =>
  invoke<string>("download_preset", { id });

/**
 * 取消当前下载——引擎包与预设模型**共用同一个通道**。
 *
 * 全项目只有这一个取消入口。早先另有一个 `cancelPresetDownload`，
 * 名字暗示它只管模型，而通道其实是共用的——两个名字指向同一件事，
 * 只会让调用方犹豫该用哪个。合并成一个。
 *
 * 取消后已下载的部分**保留**，下次会续传。
 */
export const cancelDownload = () => invoke<void>("cancel_download");

/**
 * 删除某个预设未完成的下载（`.part` + `.part.meta`），返回释放的字节数。
 *
 * 与取消的区别：取消是「先停一下」，保留数据以便续传；删除是「不要了」。
 *
 * **必须提供这个入口**：`.part` 不进模型列表、启动时不扫描，用户没有
 * 任何别的办法看到并清理它。没有它，取消一次 8 GB 下载就等于永久占
 * 8 GB 磁盘且无从察觉。
 */
export const deletePresetPartial = (id: string) =>
  invoke<number>("delete_preset_partial", { id });

/** 预设下载进度。 */
export const onPresetProgress = (
  cb: (p: PresetProgress) => void,
): Promise<UnlistenFn> =>
  listen<PresetProgress>("preset://progress", (e) => cb(e.payload));

/**
 * 翻译缓存的当前状态。
 *
 * `persistent` 为 false 表示落盘不可用（目录不可写等），
 * 此时缓存退化成纯内存，重启即失效——设置面板要把这点说清楚，
 * 否则用户会以为「翻过的下次还在」，结果白等。
 */
export interface CacheStatus {
  entries: number;
  bytes: number;
  persistent: boolean;
}

export const getCacheStatus = () => invoke<CacheStatus>("cache_status");
/** 清空翻译缓存（内存 + 磁盘），返回被清掉的条数。 */
export const clearTranslateCache = () => invoke<number>("clear_translate_cache");
/** 思考档位表（预算数值由后端给出，前端不另抄一份）。 */
export const thinkingTiers = () => invoke<ThinkingTier[]>("thinking_tiers");

export interface ThinkingTier {
  value: ThinkingMode;
  /** 该档思考预算；-1 不限，null 表示不设置 */
  budgetTokens: number | null;
}

export const restartService = () => invoke<ServiceStatus>("restart_service");
export const translate = (request: TranslateRequest) =>
  invoke<TranslateResult>("translate", { request });
export const chat = (request: ChatRequest) =>
  invoke<string>("chat", { request });

/**
 * 把一段较早的对话压成摘要。
 *
 * 用于历史太长时避免"整段丢弃"：摘要会替换掉被压缩的那几轮，
 * 既腾出上下文，又不至于把之前定好的译法全部忘掉。
 */
export const compactHistory = (model: string, messages: ChatMessage[]) =>
  invoke<string>("compact_history", { request: { model, messages } });
export const abort = (model?: string) =>
  invoke<void>("abort", { model: model ?? null });
/** 当前**可用**显存（MB）。要引擎在磁盘上才问得到。 */
export const probeVram = () => invoke<number | null>("probe_vram");

/**
 * 硬件**总**显存（MB），不依赖引擎。
 *
 * 与 `probeVram` 的分工：那个问引擎「我现在能分到多少」，这个问系统
 * 「这块卡总共多少」。引导层还没下引擎，只能用后者判断「这档跑不跑得动」。
 *
 * `null` 表示读不到（无独显 / 不支持的平台），界面应退化成不给建议，
 * 而不是显示错的数字。
 */
export const probeTotalVram = () => invoke<number | null>("probe_total_vram");

/**
 * 物理总内存（MB），不依赖引擎。
 *
 * 与显存一起用于判断「模型跑不跑得动」：显存放不下时可以有一部分层
 * 留在内存里跑，只看显存会把这种情况误判成跑不动。
 */
export const probeTotalRam = () => invoke<number | null>("probe_total_ram");

/**
 * 内存总量与可用量（MB），一次取回。
 *
 * 界面上的内存条需要「总 − 可用」。分成两次命令会取到不同时刻的值，
 * 进度条会跳；后端用 `sysinfo` 一次读出两个数。
 * `null` 表示读不到——界面应显示「读取不到」，不要猜。
 */
export interface MemoryInfo {
  totalMb: number;
  availableMb: number;
}
export const probeRam = () => invoke<MemoryInfo | null>("probe_ram");
export const listSessions = () => invoke<Session[]>("list_sessions");
export const saveSession = (session: Session) =>
  invoke<void>("save_session", { session });
export const deleteSession = (id: string) =>
  invoke<void>("delete_session", { id });
/** 清空全部历史会话，返回删掉的数量。 */
export const deleteAllSessions = () => invoke<number>("delete_all_sessions");
export const getGlossary = () => invoke<GlossaryEntry[]>("get_glossary");
export const setGlossary = (entries: GlossaryEntry[]) =>
  invoke<void>("set_glossary", { entries });

export interface TokenPayload {
  requestId: string;
  token: string;
}

export const onRuntimeProgress = (
  cb: (p: RuntimeProgress) => void,
): Promise<UnlistenFn> =>
  listen<RuntimeProgress>("runtime://progress", (e) => cb(e.payload));

export const onToken = (cb: (p: TokenPayload) => void): Promise<UnlistenFn> =>
  listen<TokenPayload>("llm://token", (e) => cb(e.payload));

/** 思维链增量。只在对话页用（翻译固定关闭思考），用来显示"模型还在想"。 */
export const onReasoning = (cb: (p: TokenPayload) => void): Promise<UnlistenFn> =>
  listen<TokenPayload>("llm://reasoning", (e) => cb(e.payload));

/** 一次补全的 token 用量，来自 llama.cpp 的 usage 统计。 */
export interface UsagePayload {
  requestId: string;
  promptTokens: number;
  completionTokens: number;
  /** 命中缓存、无需重算的前缀 token 数 */
  cachedTokens: number;
  /** 该模型当前的上下文上限 */
  ctxSize: number;
  /** 本次请求耗时（毫秒），用于算 tokens/s */
  elapsedMs: number;
}

export const onUsage = (cb: (p: UsagePayload) => void): Promise<UnlistenFn> =>
  listen<UsagePayload>("llm://usage", (e) => cb(e.payload));

export const onTranslateProgress = (
  cb: (p: TranslateProgress) => void,
): Promise<UnlistenFn> =>
  listen<TranslateProgress>("translate://progress", (e) => cb(e.payload));

export const onQualityReport = (
  cb: (p: QualityReport) => void,
): Promise<UnlistenFn> =>
  listen<QualityReport>("translate://quality", (e) => cb(e.payload));

export const onModelState = (
  cb: (p: ModelStatePayload) => void,
): Promise<UnlistenFn> =>
  listen<ModelStatePayload>("model://state-changed", (e) => cb(e.payload));

export type BatchStage =
  | "reading"
  | "translating"
  | "writing"
  | "done"
  | "failed"
  | "skipped";

export interface BatchFileProgress {
  path: string;
  index: number;
  total: number;
  stage: BatchStage;
  done: number;
  segments: number;
  failedSegments: number;
  /**
   * 有多少段是从上次的进度里恢复的。
   *
   * 大于 0 表示这次是「接着上次翻」，界面要告诉用户，
   * 否则他会奇怪为什么一开始就是 250/300。
   */
  resumedSegments?: number;
  output?: string;
  error?: string;
}

export interface BatchSummary {
  total: number;
  succeeded: number;
  failed: number;
  skipped: number;
  failedSegments: number;
  /** 首次质检就检出问题的片段数（含已修好的） */
  qualitySegments: number;
  /** 其中重试后修好的片段数 */
  qualityFixed: number;
  elapsedMs: number;
  cancelled: boolean;
}

export interface BatchRequest {
  model: string;
  source: string;
  target: string;
  paths: string[];
}

export const batchTranslate = (request: BatchRequest) =>
  invoke<BatchSummary>("batch_translate", { request });
export const batchCancel = () => invoke<void>("batch_cancel");

export const onBatchProgress = (
  cb: (p: BatchFileProgress) => void,
): Promise<UnlistenFn> =>
  listen<BatchFileProgress>("batch://progress", (e) => cb(e.payload));

/** 选择待翻译的文本 / 字幕文件。 */
export const pickBatchFiles = () =>
  open({
    multiple: true,
    filters: [{ name: "文本与字幕", extensions: ["txt", "md", "srt"] }],
  });

/** 批量翻译支持的扩展名（含点号，小写）。 */
export const BATCH_EXTENSIONS = [".txt", ".md", ".srt"] as const;

/** 过滤出支持的文件并去重，保持原有顺序。 */
export function filterBatchPaths(paths: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];

  for (const path of paths) {
    const lower = path.toLowerCase();
    if (!BATCH_EXTENSIONS.some((ext) => lower.endsWith(ext))) continue;
    if (seen.has(path)) continue;
    seen.add(path);
    out.push(path);
  }

  return out;
}

/** 从完整路径取出文件名（兼容 Windows 与 POSIX 分隔符）。 */
export function baseName(path: string): string {
  const at = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return at >= 0 ? path.slice(at + 1) : path;
}

/** 生成一次请求的标识，用于把流式 token 归属到具体请求。 */
export function newRequestId(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c?.randomUUID) return c.randomUUID();
  return `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}
