import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as ipc from "../lib/ipc";
import { ModelDrawer } from "./ModelDrawer";

/**
 * 设置抽屉组件测试。
 *
 * 这是本项目首次给 `ModelDrawer.tsx` 补测试 —— 它 912 行、
 * 负责读写用户配置，此前零测试（技术债 #8）。
 *
 * 为什么这里风险高：它改的是落盘的 `config.json`。
 * 逻辑写错不会崩溃，而是**静默写坏用户数据** ——
 * 比如 TTL 填 0 变成「永不卸载」、上下文设成越界值。
 */

vi.mock("../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/ipc")>();
  return {
    ...actual,
    getRuntimeStatus: vi.fn(async () => ({
      installed: true,
      version: "b11714",
      backend: "manual",
      serverPath: "D:/llama/llama-server.exe",
      modelsDir: "D:/models",
    })),
    getSettings: vi.fn(async () => ({
      ttlSecs: 120,
      gpuLayers: 1001,
      pinned: [],
      modelsDir: "D:/models",
      running: true,
      port: 8080,
    })),
    listLocalModels: vi.fn(async () => [
      {
        id: "Hy-MT2-1.8B-Q8_0",
        path: "D:/models/Hy-MT2-1.8B-Q8_0.gguf",
        size: 1_908_528_192,
        contextSize: 4096,
        thinking: "off",
        family: "Hy-MT2",
        style: "chinese-official",
        chatCapable: true,
        chatOverride: null,
      },
    ]),
    listRouterModels: vi.fn(async () => []),
    getGlossary: vi.fn(async () => []),
    setGlossary: vi.fn(async () => {}),
    setTtl: vi.fn(async () => {}),
    setGpuLayers: vi.fn(async () => {}),
    setModelsDir: vi.fn(async () => "D:/models"),
    setPinned: vi.fn(async () => []),
    setModelParams: vi.fn(async () => ({
      ctx: 4096,
      thinking: "off" as const,
    })),
    setModelChatCapable: vi.fn(async () => {}),
    setManualRuntime: vi.fn(async () => ({})),
    discoverRuntimes: vi.fn(async () => []),
    probeVram: vi.fn(async () => null),
    openModelsDir: vi.fn(async () => "D:/models"),
    restartService: vi.fn(async () => ({
      running: true,
      port: 8080,
      loaded: [],
      modelsDir: "D:/models",
    })),
    abort: vi.fn(async () => {}),
    getCacheStatus: vi.fn(async () => ({
      entries: 5,
      bytes: 1024,
      persistent: true,
    })),
    clearTranslateCache: vi.fn(async () => 5),
    openDataSubdir: vi.fn(async () => ""),
    openEngineDir: vi.fn(async () => ""),
    getAppInfo: vi.fn(async () => ({
      os: "windows",
      arch: "x86_64",
      version: "0.1.0",
      dataDir: "C:/data",
      modelsDir: "D:/models",
      runtimeVersion: "b11714",
    })),
    syncWindowTheme: vi.fn(async () => {}),
  };
});

function renderDrawer(props: Partial<Parameters<typeof ModelDrawer>[0]> = {}) {
  return render(
    <ModelDrawer
      open
      onClose={() => {}}
      onChanged={() => {}}
      {...props}
    />,
  );
}

describe("ModelDrawer · 设置读写", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("打开时读取设置并显示 TTL", async () => {
    renderDrawer();
    await waitFor(() => {
      expect(ipc.getSettings).toHaveBeenCalled();
    });
    // 120 秒的 TTL 应该以某种形式出现在界面上
    await waitFor(() => {
      expect(screen.getByText(/120/)).toBeInTheDocument();
    });
  });

  it("关闭状态不渲染内容", async () => {
    renderDrawer({ open: false });
    // 抽屉 unmount 后不应有可交互的设置项
    await waitFor(() => {
      expect(screen.queryByText(/通用/)).toBeNull();
    });
  });

  it("模型列表在「模型与术语」页显示模型 id", async () => {
    const user = userEvent.setup();
    renderDrawer();

    // 模型列表不在默认的「通用」页，要先切过去
    await user.click(await screen.findByRole("tab", { name: /模型与术语/ }));

    await waitFor(() => {
      expect(screen.getByText(/Hy-MT2-1.8B-Q8_0/)).toBeInTheDocument();
    });
  });

  it("保存术语表会把解析后的条目交给后端", async () => {
    const user = userEvent.setup();
    renderDrawer();

    // 切到「模型与术语」页
    const tab = await screen.findByRole("tab", { name: /模型与术语/ });
    await user.click(tab);

    const box = await screen.findByPlaceholderText(/API=接口/);
    await user.clear(box);
    await user.type(box, "API=接口\nLLM=大语言模型");

    const save = screen.getByRole("button", { name: /保存术语表/ });
    await user.click(save);

    await waitFor(() => {
      expect(ipc.setGlossary).toHaveBeenCalledWith([
        { from: "API", to: "接口" },
        { from: "LLM", to: "大语言模型" },
      ]);
    });
  });

  it("术语表空行被忽略，不会产生空条目", async () => {
    const user = userEvent.setup();
    renderDrawer();

    const tab = await screen.findByRole("tab", { name: /模型与术语/ });
    await user.click(tab);
    const box = await screen.findByPlaceholderText(/API=接口/);
    await user.clear(box);
    // 故意混入：空行、只有左边、只有右边、没有等号的。
    // 这几种都必须被拒绝 —— 写进配置会让术语表出现无从匹配的条目。
    await user.type(box, "A=B\n\n\n=只有右边\n只有左边=\n没有等号\nC=D");

    await user.click(screen.getByRole("button", { name: /保存术语表/ }));

    await waitFor(() => {
      expect(ipc.setGlossary).toHaveBeenCalledWith([
        { from: "A", to: "B" },
        { from: "C", to: "D" },
      ]);
    });
  });
});
