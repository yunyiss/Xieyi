import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

export interface AppInfo {
  os: string;
  arch: string;
  version: string;
  dataDir: string;
  modelsDir: string;
  runtimeVersion: string | null;
}

export interface RuntimeStatus {
  installed: boolean;
  version: string | null;
  backend: string | null;
  serverPath: string | null;
  modelsDir: string;
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
}

export interface LocalModel {
  id: string;
  path: string;
  sizeMb: number;
}

export interface GlossaryEntry {
  from: string;
  to: string;
}

export interface TranslateProgress {
  done: number;
  total: number;
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
}

export interface ChatMessage {
  role: "user" | "assistant" | "system";
  content: string;
}

export interface ChatRequest {
  requestId: string;
  model: string;
  messages: ChatMessage[];
}

export interface Session {
  id: string;
  title: string;
  model: string;
  updatedAt: number;
  messages: ChatMessage[];
}

export const getAppInfo = () => invoke<AppInfo>("app_info");
export const getRuntimeStatus = () => invoke<RuntimeStatus>("runtime_status");
export const installRuntime = (backend: string) =>
  invoke<RuntimeStatus>("install_runtime", { backend });
export const setManualRuntime = (serverPath: string) =>
  invoke<RuntimeStatus>("set_manual_runtime", { serverPath });

export const getServiceStatus = () => invoke<ServiceStatus>("service_status");
export const startService = () => invoke<ServiceStatus>("start_service");
export const stopService = () => invoke<void>("stop_service");
export const listRouterModels = () => invoke<RouterModel[]>("list_router_models");
export const openModelsDir = () => invoke<string>("open_models_dir");
export const setModelsDir = (path: string) =>
  invoke<string>("set_models_dir", { path });
export const setTtl = (seconds: number) => invoke<void>("set_ttl", { seconds });
export const setGpuLayers = (layers: number) =>
  invoke<void>("set_gpu_layers", { layers });
export const setPinned = (model: string, pinned: boolean) =>
  invoke<string[]>("set_pinned", { model, pinned });
export const getSettings = () => invoke<Settings>("get_settings");
export const listLocalModels = () => invoke<LocalModel[]>("list_local_models");
export const restartService = () => invoke<ServiceStatus>("restart_service");
export const unloadModel = (model: string) =>
  invoke<void>("unload_model", { model });
export const translate = (request: TranslateRequest) =>
  invoke<string>("translate", { request });
export const chat = (request: ChatRequest) =>
  invoke<string>("chat", { request });
export const abort = (model?: string) =>
  invoke<void>("abort", { model: model ?? null });
export const probeVram = () => invoke<number | null>("probe_vram");
export const listSessions = () => invoke<Session[]>("list_sessions");
export const saveSession = (session: Session) =>
  invoke<void>("save_session", { session });
export const deleteSession = (id: string) =>
  invoke<void>("delete_session", { id });
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

export const onTranslateProgress = (
  cb: (p: TranslateProgress) => void,
): Promise<UnlistenFn> =>
  listen<TranslateProgress>("translate://progress", (e) => cb(e.payload));

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
  output?: string;
  error?: string;
}

export interface BatchSummary {
  total: number;
  succeeded: number;
  failed: number;
  skipped: number;
  failedSegments: number;
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
