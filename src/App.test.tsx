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
      partialBytes: 0,
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
      partialBytes: 0,
    })),
    getServiceStatus: vi.fn(async () => ({
      running: false,
      port: null,
      loaded: [],
      modelsDir: "D:/Model/translate",
      partialBytes: 0,
    })),
    getSettings: vi.fn(async () => ({
      ttlSecs: 120,
      gpuLayers: 1001,
      pinned: [],
      modelsDir: "D:/Model/translate",
      partialBytes: 0,
      running: false,
      port: null,
      preload: false,
      preloadModel: null,
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
      partialBytes: 0,
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
      partialBytes: 0,
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
      partialBytes: 0,
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

  /**
   * **引擎已装、模型目录为空时，不该弹引导层。**
   *
   * 这条锁住一个真实反馈的 bug：用户配好引擎、留空模型目录进了主界面，
   * 关掉应用重新打开又被打回全屏引导层——因为「没有模型」又成立了一次，
   * 用户以为配置丢了。
   *
   * 根因是启动判定用了 `ready`（引擎 && 有模型），而
   * `lib/readiness.ts` 里明确写着进主界面只看引擎（`canEnter`）：
   *
   * > 模型目录本来就允许空的……把这些用户挡在门外没有任何好处，
   * > 而且会形成死循环：不放模型就进不去，进不去就看不到模型目录在哪。
   *
   * 变异验证：把 `App.tsx` 里的 `!r.canEnter` 改回 `!r.ready`，
   * 本测试失败。
   *
   * 注意这条与前一条的区别：前一条「有模型」两种情况都通过，
   * **只有这一条**能区分 canEnter 与 ready。
   */
  it("引擎已装但模型目录为空时，也不弹引导层", async () => {
    vi.mocked(ipc.countLocalModels).mockResolvedValue(0);

    render(<App />);

    // 等启动判定跑完
    await waitFor(() => {
      expect(ipc.countLocalModels).toHaveBeenCalled();
    });

    // 关键断言：没模型也不能把人拦在门外
    expect(screen.queryByText(/稍后配置/)).toBeNull();
  });
});
