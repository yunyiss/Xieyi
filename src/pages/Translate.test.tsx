import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as ipc from "../lib/ipc";
import { Translate } from "./Translate";

/**
 * 翻译页组件测试：不连 Tauri，`lib/ipc` 全部打桩。
 * 重点是确认界面选择（风格/语言/模型）确实进了请求，译文能落回页面。
 */
/**
 * onModelState 的回调，由 mock 捕获。
 * 测试用它模拟"引擎报告模型状态变化"。
 */
let modelStateCb: ((p: { model: string; state: string }) => void) | null = null;

vi.mock("../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/ipc")>();

  const noop = async () => () => {};

  /**
   * 一个最小可用的模型画像。
   *
   * 页面启动改用 `listLocalModels`（纯扫目录，不拉引擎），
   * 所以桩要给完整对象——只需要 `id` 能填进下拉，其余字段给默认值。
   */
  const LOCAL_MODEL = {
    id: "demo.gguf",
    path: "C:/models/demo.gguf",
    sizeMb: 1024,
    ctxSize: 4096,
    trainedCtx: 4096,
    supportsThinking: false,
    thinking: "off" as const,
    chatCapable: true,
    family: "通用",
    unsupportedReason: null,
    chatAuto: true,
    chatOverridden: false,
  };

  return {
    ...actual,
    getRuntimeStatus: vi.fn(async () => ({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "C:/llama/llama-server.exe",
      modelsDir: "C:/models",
    })),
    startService: vi.fn(async () => ({
      running: true,
      port: 8080,
      loaded: [],
      modelsDir: "C:/models",
    })),
    getServiceStatus: vi.fn(async () => ({
      running: true,
      port: 8080,
      loaded: [],
      modelsDir: "C:/models",
    })),
    stopService: vi.fn(async () => {}),
    listLocalModels: vi.fn(async () => [LOCAL_MODEL]),
    newRequestId: vi.fn(() => "req-1"),
    translate: vi.fn(),
    abort: vi.fn(async () => {}),
    installRuntime: vi.fn(),
    setManualRuntime: vi.fn(),
    openModelsDir: vi.fn(),
    setModelsDir: vi.fn(),
    // 捕获回调，供「状态不被重置」那条测试驱动
    onModelState: vi.fn(async (cb: (p: unknown) => void) => {
      modelStateCb = cb as typeof modelStateCb;
      return () => {};
    }),
    onQualityReport: vi.fn(noop),
    onRuntimeProgress: vi.fn(noop),
    onToken: vi.fn(noop),
    onTranslateProgress: vi.fn(noop),
  };
});

const translateMock = vi.mocked(ipc.translate);

beforeEach(() => {
  vi.clearAllMocks();
});

describe("Translate", () => {
  it("把所选语言带进请求，并把译文显示出来", async () => {
    translateMock.mockResolvedValue({ text: "你好，世界", fromCache: false });

    const user = userEvent.setup();
    render(<Translate />);

    const run = await screen.findByRole("button", { name: "开始翻译" });
    await waitFor(() => expect(run).toBeEnabled());

    await user.type(
      screen.getByPlaceholderText("粘贴或输入要翻译的内容…"),
      "hello world",
    );

    await user.click(run);

    expect(await screen.findByText("你好，世界")).toBeInTheDocument();
    expect(translateMock).toHaveBeenCalledWith(
      expect.objectContaining({
        requestId: "req-1",
        model: "demo.gguf",
        source: "auto",
        target: "zh",
      }),
    );
  });

  it("命中缓存时提示「来自缓存」，且随后自动消失", async () => {
    translateMock.mockResolvedValue({ text: "你好，世界", fromCache: true });

    const user = userEvent.setup();
    render(<Translate />);

    const run = await screen.findByRole("button", { name: "开始翻译" });
    await waitFor(() => expect(run).toBeEnabled());
    await user.type(
      screen.getByPlaceholderText("粘贴或输入要翻译的内容…"),
      "hello world",
    );
    await user.click(run);

    // 提示出现
    expect(await screen.findByText("来自缓存")).toBeInTheDocument();
    // 用完即走，不常驻标题栏
    await waitFor(() => expect(screen.queryByText("来自缓存")).toBeNull(), {
      timeout: 3000,
    });
  });

  it("提示会随下次翻译更新（不残留上一次的）", async () => {
    translateMock.mockResolvedValue({ text: "第一版", fromCache: true });

    const user = userEvent.setup();
    render(<Translate />);

    const run = await screen.findByRole("button", { name: "开始翻译" });
    await waitFor(() => expect(run).toBeEnabled());
    await user.type(
      screen.getByPlaceholderText("粘贴或输入要翻译的内容…"),
      "hello",
    );
    await user.click(run);
    expect(await screen.findByText("来自缓存")).toBeInTheDocument();

    // 第二次不命中：提示要消失，不能挂着上一次的
    translateMock.mockResolvedValue({ text: "第二版", fromCache: false });
    await user.click(screen.getByRole("button", { name: "重新翻译" }));
    await waitFor(() => {
      expect(screen.queryByText("来自缓存")).toBeNull();
    });
  });

  it("「重新翻译」会带上 skipCache=true", async () => {
    translateMock.mockResolvedValue({ text: "你好，世界", fromCache: false });

    const user = userEvent.setup();
    render(<Translate />);

    const run = await screen.findByRole("button", { name: "开始翻译" });
    await waitFor(() => expect(run).toBeEnabled());
    await user.type(
      screen.getByPlaceholderText("粘贴或输入要翻译的内容…"),
      "hello world",
    );
    await user.click(run);
    await screen.findByText("你好，世界");

    translateMock.mockClear();
    await user.click(screen.getByRole("button", { name: "重新翻译" }));

    // 关键：必须显式跳过缓存，否则点了跟没点一样
    expect(translateMock).toHaveBeenCalledWith(
      expect.objectContaining({ skipCache: true }),
    );
  });

  it("空输入不会发请求", async () => {
    const user = userEvent.setup();
    render(<Translate />);

    const run = await screen.findByRole("button", { name: "开始翻译" });
    await waitFor(() => expect(run).toBeEnabled());

    await user.click(run);

    expect(translateMock).not.toHaveBeenCalled();
    expect(screen.getByText("请输入要翻译的内容")).toBeInTheDocument();
  });

  it("翻译页不过滤模型：翻译专用模型也必须能选", async () => {
    // **翻译页与对话页的过滤策略故意不同。**
    //
    // 对话页要过滤掉 `chatCapable === false` 的模型（translategemma 这类
    // 单轮翻译器会把提问本身翻掉）。但翻译页恰恰**只该用这些模型**——
    // 把 Hy-MT2 / translategemma 从翻译页过滤掉，等于这个工具没法用。
    //
    // 这条测试锁住这一点：即使 chatCapable 为 false，翻译页也要列出来。
    // 变异验证：给 Translate 加上 `filter(m => m.chatCapable)` → 本条失败。
    vi.mocked(ipc.listLocalModels).mockResolvedValue([
      {
        id: "translategemma-4b.gguf",
        path: "C:/models/translategemma-4b.gguf",
        sizeMb: 3800,
        ctxSize: 4096,
        trainedCtx: 131072,
        supportsThinking: false,
        thinking: "off",
        // 关键：对话页不能用
        chatCapable: false,
        family: "TranslateGemma",
        unsupportedReason: null,
        chatAuto: false,
        chatOverridden: false,
      },
      {
        id: "Hy-MT2-7B.gguf",
        path: "C:/models/Hy-MT2-7B.gguf",
        sizeMb: 4600,
        ctxSize: 4096,
        trainedCtx: 32768,
        supportsThinking: false,
        thinking: "off",
        chatCapable: true,
        family: "Hy-MT2",
        unsupportedReason: null,
        chatAuto: true,
        chatOverridden: false,
      },
    ]);

    render(<Translate />);

    const run = await screen.findByRole("button", { name: "开始翻译" });
    await waitFor(() => expect(run).toBeEnabled());

    // 默认选中的是第一个（按 id 排序后），说明翻译专用模型也在列表里
    // 且可被选中——不是被过滤掉了
    expect(screen.getByTitle(/translategemma-4b/)).toBeInTheDocument();
  });
});
