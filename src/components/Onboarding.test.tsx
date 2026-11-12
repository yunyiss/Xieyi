import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import * as ipc from "../lib/ipc";
import { Onboarding } from "./Onboarding";

/**
 * 首启引导层测试。
 *
 * 它是新用户唯一必经之路：这里卡住，用户看到的就是一片空白，
 * 会直接放弃。所以重点测「能不能顺利走完」，而不是下载细节。
 *
 * 不测的：真实下载/校验/解压（那是后端职责，已有单测）、
 * 进度条百分比动画（纯展示）。
 */

vi.mock("../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/ipc")>();
  return {
    ...actual,
    getRuntimeStatus: vi.fn(async () => ({
      installed: false,
      version: null,
      backend: null,
      serverPath: null,
      modelsDir: "D:/models",
    })),
    discoverRuntimes: vi.fn(async () => []),
    listRouterModels: vi.fn(async () => []),
    installRuntime: vi.fn(async () => ({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
    })),
    setManualRuntime: vi.fn(async () => ({
      installed: true,
      version: "manual",
      backend: "manual",
      serverPath: "D:/llama/llama-server.exe",
      modelsDir: "D:/models",
    })),
    setModelsDir: vi.fn(async () => "D:/models"),
    restartService: vi.fn(async () => ({
      running: true,
      port: 8080,
      loaded: [],
      modelsDir: "D:/models",
    })),
    onRuntimeProgress: vi.fn(async () => () => {}),
  };
});

function renderOnboarding(onDone = vi.fn(), onChanged = vi.fn()) {
  render(<Onboarding onDone={onDone} onChanged={onChanged} />);
  return { onDone, onChanged };
}

/**
 * 窗口高度自适应用的类名必须在 CSS 里有定义。
 *
 * 这种“类名拼错”不会报错，只会让自适应静默失效：
 * 小窗口下依然溢出，但没人知道是哪个类没生效。
 * 这里做静态对账：JSX 用到的必须在 CSS 里存在。
 */
/**
 * 窗口高度自适应用的类名必须在 CSS 里有定义。
 *
 * 这种“类名拼错”不会报错，只会让自适应静默失效：
 * 小窗口下依然溢出，但没人知道是哪个类没生效。
 */
describe("Onboarding · 自适应类名一致性", () => {
  it("媒体查询覆盖了 JSX 用到的所有 onboarding-* 类", async () => {
    const { ONBOARDING_CLASSES } = await import("./onboardingClasses");
    // 直接读 CSS 源文件做静态对账：类名拼错不会报错，
    // 只会让自适应静默失效，这里把它变成测试失败。
    const css = readFileSync("src/index.css", "utf-8");

    expect(ONBOARDING_CLASSES.length).toBeGreaterThan(0);

    // 精确匹配选择器：.onboarding-titleX 不该被当成 .onboarding-title 存在
    const missing = ONBOARDING_CLASSES.filter(
      (c) => !new RegExp("\\." + c + "(?![-a-zA-Z0-9])").test(css),
    );
    expect(
      missing,
      `这些类在 CSS 里没定义：${missing.join(", ")}`,
    ).toEqual([]);
  });
});

describe("Onboarding", () => {
  beforeEach(() => {
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: false,
      version: null,
      backend: null,
      serverPath: null,
      modelsDir: "D:/models",
    });
    vi.mocked(ipc.discoverRuntimes).mockResolvedValue([]);
    vi.mocked(ipc.listRouterModels).mockResolvedValue([]);
  });

  it("渲染引导层且不阻塞（能拿到「稍后配置」）", async () => {
    renderOnboarding();
    // 新用户最需要的逃生口：不想现在配也能进主界面
    await waitFor(() => {
      expect(screen.getByText(/稍后配置/)).toBeInTheDocument();
    });
  });

  it("点「稍后配置」会关闭引导层", async () => {
    const user = userEvent.setup();
    const { onDone } = renderOnboarding();

    const skip = await screen.findByText(/稍后配置/);
    await user.click(skip);

    expect(onDone).toHaveBeenCalled();
  });

  it("发现本机已有引擎时列出来供选择", async () => {
    vi.mocked(ipc.discoverRuntimes).mockResolvedValue([
      {
        path: "D:/llama-b11714/llama-server.exe",
        label: "llama-b11714-bin-win-cuda-13.4-x64",
        backend: "cuda",
        version: "b11714",
      },
    ]);
    renderOnboarding();

    await waitFor(() => {
      expect(screen.getByText(/llama-b11714/)).toBeInTheDocument();
    });
  });

  it("选中已有引擎后调用 setManualRuntime 而不是下载", async () => {
    const user = userEvent.setup();
    vi.mocked(ipc.discoverRuntimes).mockResolvedValue([
      {
        path: "D:/llama-b11714/llama-server.exe",
        label: "llama-b11714-bin-win-cuda-13.4-x64",
        backend: "cuda",
        version: "b11714",
      },
    ]);
    const { onChanged } = renderOnboarding();

    const item = await screen.findByText(/llama-b11714/);
    await user.click(item);

    await waitFor(() => {
      expect(ipc.setManualRuntime).toHaveBeenCalledWith(
        "D:/llama-b11714/llama-server.exe",
      );
    });
    // 关键：有现成引擎就不该去下载 30 MB
    expect(ipc.installRuntime).not.toHaveBeenCalled();
    expect(onChanged).toHaveBeenCalled();
  });

  it("没有候选引擎时提供下载入口", async () => {
    renderOnboarding();
    await waitFor(() => {
      expect(screen.getByText(/下载推理引擎/)).toBeInTheDocument();
    });
  });

  it("点下载会调用 installRuntime", async () => {
    const user = userEvent.setup();
    renderOnboarding();

    const dl = await screen.findByText(/下载推理引擎/);
    await user.click(dl);

    await waitFor(() => {
      expect(ipc.installRuntime).toHaveBeenCalled();
    });
  });

  it("已选目录但还没扫到模型时，也允许开始使用", async () => {
    // 扫描要读每个 GGUF 的元数据，模型多的用户会等几秒。
    // 用户已经选了目录，没理由让他等扫描完。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/Model/translate",
    });
    // 扫描还没出结果
    vi.mocked(ipc.listRouterModels).mockResolvedValue([]);

    renderOnboarding();

    await waitFor(() => {
      const btn = screen.getByRole("button", { name: /开始使用/ });
      expect(btn).toBeEnabled();
    });
  });

  it("扫描中显示「正在扫描」而不是默默等待", async () => {
    // 不给提示时用户只看到按钮灰着，不知道在等什么。
    // 注意：只有引擎已装时才会去列模型（未装时直接跳过）。
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/Model/translate",
    });
    let release: (v: never[]) => void = () => {};
    vi.mocked(ipc.listRouterModels).mockImplementation(
      () => new Promise((res) => { release = res as never; }),
    );

    renderOnboarding();

    await waitFor(() => {
      expect(screen.getByText(/正在扫描/)).toBeInTheDocument();
    });

    // 扫描完成后提示切换成结果
    release([]);
    await waitFor(() => {
      expect(screen.queryByText(/正在扫描/)).toBeNull();
    });
  });

  it("引擎少于等于 7 个时不限高（不出现滚动条）", async () => {
    // 多数用户只有 1-3 个引擎，滚动条纯属噪音
    vi.mocked(ipc.discoverRuntimes).mockResolvedValue(
      Array.from({ length: 3 }, (_, i) => ({
        path: `D:/llama-${i}/llama-server.exe`,
        label: `engine-${i}`,
        backend: "cuda",
        version: "b11714",
      })),
    );
    renderOnboarding();

    const list = await screen.findByRole("list");
    expect(list.style.maxHeight).toBe("");
  });

  it("引擎超过 7 个时限高滚动", async () => {
    vi.mocked(ipc.discoverRuntimes).mockResolvedValue(
      Array.from({ length: 9 }, (_, i) => ({
        path: `D:/llama-${i}/llama-server.exe`,
        label: `engine-${i}`,
        backend: "cuda",
        version: "b11714",
      })),
    );
    renderOnboarding();

    const list = await screen.findByRole("list");
    await waitFor(() => {
      expect(list.style.maxHeight).not.toBe("");
    });
    expect(list.style.overflowY).toBe("auto");
  });

  it("已装引擎时不再重复安装", async () => {
    // 二次打开引导层（或用户手动重开）时，不该再触发一次下载
    vi.mocked(ipc.getRuntimeStatus).mockResolvedValue({
      installed: true,
      version: "b11714",
      backend: "vulkan",
      serverPath: "D:/runtime/llama-server.exe",
      modelsDir: "D:/models",
    });
    renderOnboarding();

    await waitFor(() => {
      expect(ipc.getRuntimeStatus).toHaveBeenCalled();
    });
    expect(ipc.installRuntime).not.toHaveBeenCalled();
  });
});
