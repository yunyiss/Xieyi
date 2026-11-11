import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as ipc from "../lib/ipc";
import { Translate } from "./Translate";

/**
 * 翻译页组件测试：不连 Tauri，`lib/ipc` 全部打桩。
 * 重点是确认界面选择（风格/语言/模型）确实进了请求，译文能落回页面。
 */
vi.mock("../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/ipc")>();

  const noop = async () => () => {};

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
    stopService: vi.fn(async () => {}),
    listRouterModels: vi.fn(async () => [{ id: "demo.gguf", state: "loaded" }]),
    newRequestId: vi.fn(() => "req-1"),
    translate: vi.fn(),
    abort: vi.fn(async () => {}),
    installRuntime: vi.fn(),
    setManualRuntime: vi.fn(),
    openModelsDir: vi.fn(),
    setModelsDir: vi.fn(),
    onModelState: vi.fn(noop),
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
});
