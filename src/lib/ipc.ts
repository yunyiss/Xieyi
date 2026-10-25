import { invoke } from "@tauri-apps/api/core";

export interface AppInfo {
  os: string;
  arch: string;
  version: string;
  dataDir: string;
  modelsDir: string;
  runtimeVersion: string | null;
}

/** 读取运行环境信息（平台、数据目录、运行时状态）。 */
export function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("app_info");
}
