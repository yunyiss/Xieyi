import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

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

export const onModelState = (
  cb: (p: { model: string; state: string }) => void,
): Promise<UnlistenFn> =>
  listen<{ model: string; state: string }>("model://state-changed", (e) =>
    cb(e.payload),
  );

/** 生成一次请求的标识，用于把流式 token 归属到具体请求。 */
export function newRequestId(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c?.randomUUID) return c.randomUUID();
  return `${Date.now()}-${Math.random().toString(16).slice(2)}`;
}
