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
    translateMock.mockResolvedValue("你好，世界");

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
