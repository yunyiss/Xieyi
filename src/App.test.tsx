import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as ipc from "./lib/ipc";
import App from "./App";

/**
 * 启动路径的测试。
 *
 * 这里只测一件事，但它很重要：**启动时不能拉起推理引擎**。
 *
 * ## 背景
 *
 * 曾经启动时调 `listRouterModels` 来判断"有没有模型"，那个命令内部是
 * `ensure()`——引擎没跑就立刻 spawn 一个。实测日志：
 *
 * ```text
 * 02:09:38  主窗口已创建（可见性 true）
 * 02:09:44  引擎启动
 * ```
 *
 * 窗口在 6 秒前就可见了，这 6 秒里页面什么内容都没有，用户看到的就是
 * "启动白屏一会儿"。而用户可能只是想看一眼界面。
 *
 * 现在改用 `countLocalModels`（纯扫目录）。这条测试锁住它：
 * 启动过程中**不能**出现任何会触发引擎的调用。
 */
vi.mock("./lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./lib/ipc")>();
  return {
    ...actual,
    getAppInfo: vi.fn(async () => ({
      os: "windows",
      arch: "x86_64",
      version: "0.1.0",
      dataDir: "C:/data",
      modelsDir: "D:/models",
      modelsDirConfigured: true,
      runtimeVersion: "b11714",
      portable: false,
    })),
    getRuntimeStatus: vi.fn(async () => ({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/Model/translate",
    })),
    getServiceStatus: vi.fn(async () => ({
      running: false,
      port: null,
      loaded: [],
      modelsDir: "D:/Model/translate",
    })),
    getSettings: vi.fn(async () => ({
      ttlSecs: 120,
      gpuLayers: 1001,
      pinned: [],
      modelsDir: "D:/Model/translate",
      running: false,
      port: null,
    })),
    countLocalModels: vi.fn(async () => 5),
    // 必须显式打桩：否则 `...actual` 带进来的是真实函数，
    // 对真实函数调 `expect(...).not.toHaveBeenCalled()` 会抛
    // "[Function startService] is not a spy"。
    startService: vi.fn(async () => ({
      running: true,
      port: 8080,
      loaded: [],
      modelsDir: "D:/Model/translate",
    })),
    listRouterModels: vi.fn(async () => []),
    listSessions: vi.fn(async () => []),
    getGlossary: vi.fn(async () => []),
    listLocalModels: vi.fn(async () => []),
    probeVram: vi.fn(async () => null),
    discoverRuntimes: vi.fn(async () => []),
    onModelState: vi.fn(async () => () => {}),
    onToken: vi.fn(async () => () => {}),
    onReasoning: vi.fn(async () => () => {}),
    onUsage: vi.fn(async () => () => {}),
    onTranslateProgress: vi.fn(async () => () => {}),
    onQualityReport: vi.fn(async () => () => {}),
    onRuntimeProgress: vi.fn(async () => () => {}),
    onBatchProgress: vi.fn(async () => () => {}),
  };
});

describe("App · 启动路径", () => {
  beforeEach(() => {
    // **不要用 vi.clearAllMocks()**：它会连 mock 的**实现**一起清掉，
    // 于是 `countLocalModels` 变成返回 undefined，App 拿到 NaN 就
    // 判成"没模型"，测试与真实行为对不上。
    //
    // 只清调用记录（mockClear），保留工厂里给的默认实现。
    vi.clearAllMocks();
    // 显式恢复本文件依赖的默认实现
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/Model/translate",
    });
    vi.mocked(ipc.countLocalModels).mockResolvedValue(5);
  });

  it("启动时用 countLocalModels 数模型，不碰会拉起引擎的接口", async () => {
    render(<App />);

    // 应当走轻量的计数命令
    await waitFor(() => {
      expect(ipc.countLocalModels).toHaveBeenCalled();
    });

    // 而**不能**走这两个：
    //   listRouterModels → 内部 ensure() → spawn() 引擎
    //   startService     → 直接起引擎
    expect(ipc.listRouterModels).not.toHaveBeenCalled();
    expect(ipc.startService).not.toHaveBeenCalled();
  });

  it("引擎未安装时弹引导层（此时不该去数模型）", async () => {
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: false,
      version: null,
      backend: null,
      serverPath: null,
      modelsDir: "D:/models",
    });

    render(<App />);

    // 引导层出现（它有「稍后配置」按钮）
    expect(await screen.findByText(/稍后配置/)).toBeInTheDocument();
    expect(ipc.countLocalModels).not.toHaveBeenCalled();
  });

  it("有模型时不弹引导层", async () => {
    vi.mocked(ipc.countLocalModels).mockResolvedValue(3);

    render(<App />);

    await waitFor(() => {
      expect(ipc.countLocalModels).toHaveBeenCalled();
    });
    // 引导层未出现：页面上不该有「稍后配置」
    expect(screen.queryByText(/稍后配置/)).toBeNull();
  });
});
